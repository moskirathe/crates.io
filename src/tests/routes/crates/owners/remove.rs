use crate::builders::{CrateBuilder, UserBuilder};
use crate::util::{RequestHelper, TestApp};
use crates_io::models::{CrateOwner, NewEmail};
use crates_io_github::{GitHubOrganization, GitHubTeam, GitHubTeamMembership, MockGitHubClient};
use insta::assert_snapshot;

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
        .delete_with_body::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"Failed to parse the request body as JSON: owners[1]: expected value at line 1 column 20"}]}"#);

    // `owners` is not an array
    let input = r#"{"owners": "foo"}"#;
    let response = user
        .delete_with_body::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"422 Unprocessable Entity");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"Failed to deserialize the JSON body into the target type: owners: invalid type: string \"foo\", expected a sequence at line 1 column 16"}]}"#);

    // missing `owners` and/or `users` fields
    let input = r#"{}"#;
    let response = user
        .delete_with_body::<()>("/api/v1/crates/foo/owners", input.as_bytes())
        .await;
    assert_snapshot!(response.status(), @"422 Unprocessable Entity");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"Failed to deserialize the JSON body into the target type: missing field `owners` at line 1 column 2"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_crate() {
    let (app, _, user) = TestApp::full().with_user().await;
    app.db_new_user("bar").await;

    let response = user.remove_named_owner("unknown", "bar").await;
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

    let response = cookie.remove_named_owner("foo", "unknown").await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find owner with login `unknown`"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_team() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie
        .remove_named_owner("foo", "github:unknown:unknown")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find owner with login `github:unknown:unknown`"}]}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_remove_uppercase_user() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let user2 = app.db_new_user("user2").await;
    let mut conn = app.db_conn().await;

    let krate = CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(user2.as_model().id)
        .created_by(cookie.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    let response = cookie.remove_named_owner("foo", "USER2").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"owners successfully removed","ok":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_remove_uppercase_team() {
    use mockall::predicate::*;

    let mut github_mock = MockGitHubClient::new();

    github_mock
        .expect_team_by_name()
        .with(eq("org"), eq("team"), always())
        .returning(|_, _, _| {
            Ok(GitHubTeam {
                id: 2,
                name: Some("team".to_string()),
                organization: GitHubOrganization {
                    id: 1,
                    avatar_url: None,
                },
            })
        });

    github_mock
        .expect_org_by_name()
        .with(eq("org"), always())
        .returning(|_, _| {
            Ok(GitHubOrganization {
                id: 1,
                avatar_url: None,
            })
        });

    github_mock
        .expect_team_membership()
        .with(eq(1), eq(2), eq("foo"), always())
        .returning(|_, _, _, _| {
            Ok(Some(GitHubTeamMembership {
                state: "active".to_string(),
            }))
        });

    let (app, _, cookie) = TestApp::full().with_github(github_mock).with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("crate42", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie.add_named_owner("crate42", "github:org:team").await;
    assert_snapshot!(response.status(), @"200 OK");

    let response = cookie
        .remove_named_owner("crate42", "github:ORG:TEAM")
        .await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"owners successfully removed","ok":true}"#);
}

/// Remove an owner using the `cratesio:` prefix.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_with_cratesio_prefix() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let user2 = app.db_new_user("user2").await;
    let mut conn = app.db_conn().await;

    let krate = CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(user2.as_model().id)
        .created_by(cookie.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    let response = cookie.remove_named_owner("foo", "cratesio:user2").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"owners successfully removed","ok":true}"#);
}

/// Remove an owner using the `github:` prefix.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_with_github_prefix() {
    let (app, _, cookie) = TestApp::full().with_user().await;
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

    let krate = CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(id)
        .created_by(cookie.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    let response = cookie
        .remove_named_owner("foo", "github:github_name")
        .await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"owners successfully removed","ok":true}"#);
}

/// Remove by `cratesio:` prefix with a nonexistent user.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_cratesio_prefix_unknown_user() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie
        .remove_named_owner("foo", "cratesio:nonexistent")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find user with crates.io username `nonexistent`"}]}"#);
}

/// Remove by `github:` prefix with a nonexistent login.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_github_prefix_unknown_user() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie
        .remove_named_owner("foo", "github:nonexistent")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"could not find user with GitHub login `nonexistent`"}]}"#);
}

/// When two different users with the same name (one by crates.io username,
/// one by GitHub login) are both owners, removal should require disambiguation.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_unprefixed_ambiguous_both_owners() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    // Create user A: crates.io username = "shared_name", gh_login = "gh_a"
    let user_a = UserBuilder::new()
        .with_username("shared_name")
        .with_gh_login("gh_a")
        .new_user();
    let id_a = user_a.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id_a)
        .email("a@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    // Create user B: crates.io username = "other_name", gh_login = "shared_name"
    let user_b = UserBuilder::new()
        .with_username("other_name")
        .with_gh_login("shared_name")
        .new_user();
    let id_b = user_b.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id_b)
        .email("b@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    let krate = CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    // Add both users as owners
    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(id_a)
        .created_by(cookie.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(id_b)
        .created_by(cookie.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    // Removing by "shared_name" should fail with disambiguation error
    let response = cookie.remove_named_owner("foo", "shared_name").await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    let text = response.text();
    assert!(
        text.contains("ambiguous"),
        "Expected disambiguation error, got: {text}"
    );
}

/// When two different users match but only one is an owner, unprefixed
/// removal should succeed without disambiguation.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_unprefixed_ambiguous_only_one_owner() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    // Create user A: crates.io username = "shared_name", gh_login = "gh_a"
    let user_a = UserBuilder::new()
        .with_username("shared_name")
        .with_gh_login("gh_a")
        .new_user();
    let id_a = user_a.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id_a)
        .email("a@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    // Create user B: crates.io username = "other_name", gh_login = "shared_name"
    let user_b = UserBuilder::new()
        .with_username("other_name")
        .with_gh_login("shared_name")
        .new_user();
    let id_b = user_b.insert(&conn).await.unwrap();
    NewEmail::builder()
        .user_id(id_b)
        .email("b@example.com")
        .verified(true)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    let krate = CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    // Only add user A as owner (not user B)
    CrateOwner::builder()
        .crate_id(krate.id)
        .user_id(id_a)
        .created_by(cookie.as_model().id)
        .build()
        .insert(&conn)
        .await
        .unwrap();

    // Removing by "shared_name" should succeed since only one of them is an owner
    let response = cookie.remove_named_owner("foo", "shared_name").await;
    assert_snapshot!(response.status(), @"200 OK");
    assert_snapshot!(response.text(), @r#"{"msg":"owners successfully removed","ok":true}"#);
}

/// Unknown prefix should return an error for removal too.
#[tokio::test(flavor = "multi_thread")]
async fn remove_owner_unknown_prefix() {
    let (app, _, cookie) = TestApp::full().with_user().await;
    let mut conn = app.db_conn().await;

    CrateBuilder::new("foo", cookie.as_model().id)
        .expect_build(&mut conn)
        .await;

    let response = cookie
        .remove_named_owner("foo", "gitlab:someone")
        .await;
    assert_snapshot!(response.status(), @"400 Bad Request");
    assert_snapshot!(response.text(), @r#"{"errors":[{"detail":"unknown prefix; valid prefixes are `cratesio:` and `github:`"}]}"#);
}
