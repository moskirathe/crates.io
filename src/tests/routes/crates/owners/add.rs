use crate::builders::{CrateBuilder, UserBuilder};
use crate::owners::expire_invitation;
use crate::util::{RequestHelper, TestApp};
use crates_io::models::token::{CrateScope, EndpointScope};
use crates_io::models::{CrateOwner, NewEmail};
use insta::assert_snapshot;

// This is testing Cargo functionality! ! !
// specifically functions modify_owners and add_owners
// which call the `PUT /crates/{crate_id}/owners` route
#[tokio::test(flavor = "multi_thread")]
async fn test_cargo_invite_owners() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    let new_user = app.db_new_user("cilantro").await;
    CrateBuilder::new("guacamole", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let json = owner
        .add_named_owner("guacamole", &new_user.as_model().gh_login)
        .await
        .good();

    // this ok:true field is what old versions of Cargo
    // need - do not remove unless you're cool with
    // dropping support for old versions
    assert!(json.ok);
    // msg field is what is sent and used in updated
    // version of cargo
    assert_eq!(
        json.msg,
        "user cilantro has been invited to be an owner of crate guacamole"
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_via_cookie() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user user-2 has been invited to be an owner of crate foo_crate","ok":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_via_token() {
    let (app, _, _, token) = TestApp::full().with_token().await;
    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", token.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let response = token.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user user-2 has been invited to be an owner of crate foo_crate","ok":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_via_change_owner_token() {
    let (app, _, _, token) = TestApp::full()
        .with_scoped_token(None, Some(vec![EndpointScope::ChangeOwners]))
        .await;

    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", token.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let response = token.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user user-2 has been invited to be an owner of crate foo_crate","ok":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_via_change_owner_token_with_matching_crate_scope() {
    let crate_scopes = Some(vec![CrateScope::try_from("foo_crate").unwrap()]);
    let endpoint_scopes = Some(vec![EndpointScope::ChangeOwners]);
    let (app, _, _, token) = TestApp::full()
        .with_scoped_token(crate_scopes, endpoint_scopes)
        .await;
    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", token.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let response = token.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user user-2 has been invited to be an owner of crate foo_crate","ok":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_via_change_owner_token_with_wrong_crate_scope() {
    let crate_scopes = Some(vec![CrateScope::try_from("bar").unwrap()]);
    let endpoint_scopes = Some(vec![EndpointScope::ChangeOwners]);
    let (app, _, _, token) = TestApp::full()
        .with_scoped_token(crate_scopes, endpoint_scopes)
        .await;
    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", token.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let response = token.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"403 Forbidden");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"this token does not have the required permissions to perform this action"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_via_publish_token() {
    let (app, _, _, token) = TestApp::full()
        .with_scoped_token(None, Some(vec![EndpointScope::PublishUpdate]))
        .await;

    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", token.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let response = token.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"403 Forbidden");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"this token does not have the required permissions to perform this action"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_change_without_auth() {
    let (app, anon, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    let user2 = app.db_new_user("user-2").await;
    let user2 = user2.as_model();

    let krate = CrateBuilder::new("foo_crate", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = anon.add_named_owner(&krate.name, &user2.gh_login).await;
    assert_snapshot!(response.status(), @"403 Forbidden");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"this action requires authentication"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_owner_change_with_legacy_field() {
    let (app, _, user1) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", user1.as_model().id)
        .expect_build(&mut conn)
        .await;
    app.db_new_user("user2").await;

    let input = r#"{"users": ["user2"]}"#;
    let response = user1
        .put::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user user2 has been invited to be an owner of crate foo","ok":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_owner_change_with_invalid_json() {
    let (app, _, user) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    app.db_new_user("bar").await;
    CrateBuilder::new("foo", user.as_model().id)
        .expect_build(&mut conn)
        .await;

    // incomplete input
    let input = r#"{"owners": ["foo", }"#;
    let response = user
        .put::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"Failed to parse the request body as JSON: owners[1]: expected value at line 1 column 20"}]}"#);

    // `owners` is not an array
    let input = r#"{"owners": "foo"}"#;
    let response = user
        .put::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"422 Unprocessable Entity");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"Failed to deserialize the JSON body into the target type: owners: invalid type: string \"foo\", expected a sequence at line 1 column 16"}]}"#);

    // missing `owners` and/or `users` fields
    let input = r#"{}"#;
    let response = user
        .put::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"422 Unprocessable Entity");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"Failed to deserialize the JSON body into the target type: missing field `owners` at line 1 column 2"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_already_invited_user() {
    let (app, _, _, owner) = TestApp::init().with_token().await;
    let mut conn = app.db_conn().await;

    app.db_new_user("invited_user").await;
    CrateBuilder::new("crate_name", owner.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    // Ensure no emails were sent up to this point
    assert_eq!(app.emails().await.len(), 0);

    // Invite the user the first time
    let response = owner.add_named_owner("crate_name", "invited_user").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user invited_user has been invited to be an owner of crate crate_name","ok":true}"#);

    // Check one email was sent, this will be the ownership invite email
    assert_eq!(app.emails().await.len(), 1);

    // Then invite the user a second time, the message should point out the user is already invited
    let response = owner.add_named_owner("crate_name", "invited_user").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user invited_user already has a pending invitation to be an owner of crate crate_name","ok":true}"#);

    // Check that no new email is sent after the second invitation
    assert_eq!(app.emails().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_with_existing_expired_invite() {
    let (app, _, _, owner) = TestApp::init().with_token().await;
    let mut conn = app.db_conn().await;

    app.db_new_user("invited_user").await;
    let krate = CrateBuilder::new("crate_name", owner.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    // Ensure no emails were sent up to this point
    assert_eq!(app.emails().await.len(), 0);

    // Invite the user the first time
    let response = owner.add_named_owner("crate_name", "invited_user").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user invited_user has been invited to be an owner of crate crate_name","ok":true}"#);

    // Check one email was sent, this will be the ownership invite email
    assert_eq!(app.emails().await.len(), 1);

    // Simulate the previous invite expiring
    expire_invitation(&app, krate.id).await;

    // Then invite the user a second time, a new invite is created as the old one expired
    let response = owner.add_named_owner("crate_name", "invited_user").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user invited_user has been invited to be an owner of crate crate_name","ok":true}"#);

    // Check that the email for the second invite was sent
    assert_eq!(app.emails().await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_crate() {
    let (app, _, user) = TestApp::full().with_user().await;
    app.db_new_user("bar").await;

    let response = user.add_named_owner("unknown", "bar").await;
    assert_snapshot!(response.status(), @"404 Not Found");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"crate `unknown` does not exist"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_user() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie.add_named_owner("foo", "unknown").await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find user with login `unknown`"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_team() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie
        .add_named_owner("foo", "github:unknown:unknown")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find the github team unknown/unknown. Make sure that you have the right permissions in GitHub. See https://doc.rust-lang.org/cargo/reference/publishing.html#github-permissions"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn max_invites_per_request() {
    let (app, _, _, owner) = TestApp::init().with_token().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("crate_name", owner.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let usernames = (0..11)
        .map(|i| format!("user_{i}"))
        .collect::<Vec<String>>();

    // Populate enough users in the database to submit 11 invites at once.
    for user in &usernames {
        app.db_new_user(user).await;
    }

    let response = owner.add_named_owners("crate_name", &usernames).await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"too many invites for this request - maximum 10"}]}"#);
}

/// Asserts that emails are only sent if the request succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn no_invite_emails_for_txn_rollback() {
    let (app, _, _, token) = TestApp::init().with_token().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("crate_name", token.as_model().user_id)
        .expect_build(&mut conn)
        .await;

    let mut usernames = (0..9).map(|i| format!("user_{i}")).collect::<Vec<String>>();

    // Populate enough users in the database to submit 9 good invites.
    for user in &usernames {
        app.db_new_user(user).await;
    }

    // Add an invalid username to the end of the invite list to cause the
    // request to fail.
    usernames.push("bananas".to_string());

    let response = token.add_named_owners("crate_name", &usernames).await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find user with login `bananas`"}]}"#);

    // No emails should have been sent.
    assert_eq!(app.emails().await.len(), 0);

    // Remove the bad username.
    let _ = usernames.pop();

    let response = token.add_named_owners("crate_name", &usernames).await;
    assert_snapshot!(response.status(), @"200 OK");

    // 9 emails to the good invitees should have been sent.
    assert_eq!(app.emails().await.len(), 9);
}

/// When username == gh_login (the common case), unprefixed add should work
/// without any disambiguation error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_unprefixed_no_ambiguity() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    app.db_new_user("new_owner").await;
    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = owner.add_named_owner("my_crate", "new_owner").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user new_owner has been invited to be an owner of crate my_crate","ok":true}"#);
}

/// When a user has a different crates.io username and GitHub login,
/// unprefixed add by crates.io username should return a disambiguation error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_unprefixed_disambiguation_error() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    // Create a user with different username and gh_login
    let new_user = UserBuilder::new()
        .with_username("cratesio_name")
        .with_gh_login("github_name")
        .new_user();
    let id = new_user.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id)
        .email("test@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    // Trying to add by the crates.io username should trigger disambiguation
    let response = owner
        .add_named_owner("my_crate", "cratesio_name")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    let text = response.text();
    assert!(
        text.contains("possibly ambiguous"),
        "Expected disambiguation error, got: {text}"
    );
    assert!(
        text.contains("cratesio:cratesio_name"),
        "Expected cratesio: suggestion, got: {text}"
    );
    assert!(
        text.contains("github:github_name"),
        "Expected github: suggestion, got: {text}"
    );
}

/// Using `cratesio:` prefix should look up by crates.io username only.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_with_cratesio_prefix() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    // Create a user with different username and gh_login
    let new_user = UserBuilder::new()
        .with_username("cratesio_name")
        .with_gh_login("github_name")
        .new_user();
    let id = new_user.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id)
        .email("test@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = owner
        .add_named_owner("my_crate", "cratesio:cratesio_name")
        .await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user github_name has been invited to be an owner of crate my_crate","ok":true}"#);
}

/// Using `github:` prefix should look up by GitHub login only.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_with_github_prefix() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    // Create a user with different username and gh_login
    let new_user = UserBuilder::new()
        .with_username("cratesio_name")
        .with_gh_login("github_name")
        .new_user();
    let id = new_user.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id)
        .email("test@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = owner
        .add_named_owner("my_crate", "github:github_name")
        .await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"user github_name has been invited to be an owner of crate my_crate","ok":true}"#);
}

/// `cratesio:` prefix with a nonexistent username should return an error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_cratesio_prefix_unknown_user() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = owner
        .add_named_owner("my_crate", "cratesio:nonexistent")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find user with crates.io username `nonexistent`"}]}"#);
}

/// `github:` prefix with a nonexistent login should return an error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_github_prefix_unknown_user() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = owner
        .add_named_owner("my_crate", "github:nonexistent")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find user with GitHub login `nonexistent`"}]}"#);
}

/// Unknown prefix should return an error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_unknown_prefix() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = owner
        .add_named_owner("my_crate", "gitlab:someone")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"unknown prefix; valid prefixes are `cratesio:` and `github:`"}]}"#);
}

/// Looking up by GitHub login when the crates.io username is different
/// should trigger a disambiguation error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_unprefixed_by_gh_login_disambiguation_error() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    // Create a user with different username and gh_login
    let new_user = UserBuilder::new()
        .with_username("cratesio_name")
        .with_gh_login("github_name")
        .new_user();
    let id = new_user.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id)
        .email("test@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    // Trying to add by the GitHub login (not the crates.io username) should
    // also trigger disambiguation
    let response = owner
        .add_named_owner("my_crate", "github_name")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    let text = response.text();
    assert!(
        text.contains("possibly ambiguous"),
        "Expected disambiguation error, got: {text}"
    );
    assert!(
        text.contains("cratesio:cratesio_name"),
        "Expected cratesio: suggestion, got: {text}"
    );
    assert!(
        text.contains("github:github_name"),
        "Expected github: suggestion, got: {text}"
    );
}

/// When using the `cratesio:` prefix and the user is already an owner,
/// it should return the "already an owner" error.
#[tokio::test(flavor = "multi_thread")]
async fn add_owner_already_owner_with_cratesio_prefix() {
    let (app, _, owner) = TestApp::init().with_user().await;
    let mut conn = app.db_conn().await;

    let existing_owner = app.db_new_user("existing_owner").await;
    let krate = CrateBuilder::new("my_crate", owner.as_model().id)
        .expect_build(&mut conn)
        .await;

    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(existing_owner.as_model().id)
        .created_by(owner.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    let response = owner
        .add_named_owner("my_crate", "cratesio:existing_owner")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"`existing_owner` is already an owner"}]}"#);
}
