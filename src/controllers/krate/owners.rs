//! All routes related to managing owners of a crate

use crate::controllers::helpers::authorization::Rights;
use crate::controllers::krate::CratePath;
use crate::models::krate::OwnerRemoveError;
use crate::models::{Crate, Owner, OwnerKind, Team, User};
use crate::models::{
    CrateOwner, NewCrateOwnerInvitation, NewCrateOwnerInvitationOutcome, NewTeam,
    krate::NewOwnerInvite, token::EndpointScope,
};
use crate::util::errors::{AppResult, BoxedAppError, bad_request, crate_not_found, custom};
use crate::views::EncodableOwner;
use crate::{App, app::AppState};
use crate::{auth::AuthCheck, email::EmailMessage};
use axum::Json;
use chrono::Utc;
use crates_io_encryption::TokenEncryption;
use crates_io_github::{GitHubAuth, GitHubClient, GitHubError};
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use http::StatusCode;
use http::request::Parts;
use minijinja::context;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UsersResponse {
    pub users: Vec<EncodableOwner>,
}

/// Lists crate owners.
#[utoipa::path(
    get,
    path = "/api/v1/crates/{name}/owners",
    params(CratePath),
    tag = "owners",
    responses((status = 200, description = "Successful Response", body = inline(UsersResponse))),
)]
pub async fn list_owners(state: AppState, path: CratePath) -> AppResult<Json<UsersResponse>> {
    let conn = state.db_read().await?;

    let krate = path.load_crate(&conn).await?;

    let users = krate
        .owners(&conn)
        .await?
        .into_iter()
        .map(Owner::into)
        .collect::<Vec<EncodableOwner>>();

    Ok(Json(UsersResponse { users }))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TeamsResponse {
    pub teams: Vec<EncodableOwner>,
}

/// Lists team owners of a crate.
#[utoipa::path(
    get,
    path = "/api/v1/crates/{name}/owner_team",
    params(CratePath),
    tag = "owners",
    responses((status = 200, description = "Successful Response", body = inline(TeamsResponse))),
)]
pub async fn get_team_owners(state: AppState, path: CratePath) -> AppResult<Json<TeamsResponse>> {
    let conn = state.db_read().await?;
    let krate = path.load_crate(&conn).await?;

    let teams = Team::owning(&krate, &conn)
        .await?
        .into_iter()
        .map(Owner::into)
        .collect::<Vec<EncodableOwner>>();

    Ok(Json(TeamsResponse { teams }))
}

/// Lists user owners of a crate.
#[utoipa::path(
    get,
    path = "/api/v1/crates/{name}/owner_user",
    params(CratePath),
    tag = "owners",
    responses((status = 200, description = "Successful Response", body = inline(UsersResponse))),
)]
pub async fn get_user_owners(state: AppState, path: CratePath) -> AppResult<Json<UsersResponse>> {
    let conn = state.db_read().await?;

    let krate = path.load_crate(&conn).await?;

    let users = User::owning(&krate, &conn)
        .await?
        .into_iter()
        .map(Owner::into)
        .collect::<Vec<EncodableOwner>>();

    Ok(Json(UsersResponse { users }))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ModifyResponse {
    /// A message describing the result of the operation.
    #[schema(example = "user ghost has been invited to be an owner of crate serde")]
    pub msg: String,

    #[schema(example = true)]
    pub ok: bool,
}

/// Adds crate owners.
#[utoipa::path(
    put,
    path = "/api/v1/crates/{name}/owners",
    params(CratePath),
    request_body = inline(ChangeOwnersRequest),
    security(
        ("api_token" = []),
        ("cookie" = []),
    ),
    tag = "owners",
    responses((status = 200, description = "Successful Response", body = inline(ModifyResponse))),
)]
pub async fn add_owners(
    app: AppState,
    path: CratePath,
    parts: Parts,
    Json(body): Json<ChangeOwnersRequest>,
) -> AppResult<Json<ModifyResponse>> {
    modify_owners(app, path.name, parts, body, true).await
}

/// Removes crate owners.
#[utoipa::path(
    delete,
    path = "/api/v1/crates/{name}/owners",
    params(CratePath),
    request_body = inline(ChangeOwnersRequest),
    security(
        ("api_token" = []),
        ("cookie" = []),
    ),
    tag = "owners",
    responses((status = 200, description = "Successful Response", body = inline(ModifyResponse))),
)]
pub async fn remove_owners(
    app: AppState,
    path: CratePath,
    parts: Parts,
    Json(body): Json<ChangeOwnersRequest>,
) -> AppResult<Json<ModifyResponse>> {
    modify_owners(app, path.name, parts, body, false).await
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ChangeOwnersRequest {
    /// List of owner login names to add or remove.
    ///
    /// For users, use just the username (e.g., `"octocat"`).
    /// For GitHub teams, use the format `github:org:team` (e.g., `"github:rust-lang:owners"`).
    ///
    /// To disambiguate between crates.io and GitHub usernames, you can use
    /// the `cratesio:username` or `github:username` prefix.
    #[schema(example = json!(["octocat", "github:rust-lang:owners", "cratesio:some_user"]))]
    #[serde(alias = "users")]
    owners: Vec<String>,
}

/// Represents a parsed owner login string, distinguishing between
/// prefixed (disambiguated) and unprefixed (possibly ambiguous) logins.
enum ParsedLogin<'a> {
    /// `cratesio:username` — look up only by `users.username`.
    CratesIo { username: &'a str },
    /// `github:username` — look up only by `users.gh_login`.
    GithubUser { username: &'a str },
    /// `github:org:team` — existing team behavior.
    GithubTeam { login: &'a str },
    /// No prefix — assume crates.io username, but check for ambiguity.
    Unprefixed { name: &'a str },
}

impl<'a> ParsedLogin<'a> {
    fn parse(login: &'a str) -> Result<Self, BoxedAppError> {
        if let Some(rest) = login.strip_prefix("cratesio:") {
            if rest.is_empty() {
                return Err(bad_request("missing username after `cratesio:` prefix"));
            }
            Ok(ParsedLogin::CratesIo { username: rest })
        } else if let Some(rest) = login.strip_prefix("github:") {
            if rest.is_empty() {
                return Err(bad_request("missing username after `github:` prefix"));
            }
            // If there's another colon, it's a team (github:org:team)
            if rest.contains(':') {
                Ok(ParsedLogin::GithubTeam { login })
            } else {
                Ok(ParsedLogin::GithubUser { username: rest })
            }
        } else if login.contains(':') {
            Err(bad_request(
                "unknown prefix; valid prefixes are `cratesio:` and `github:`",
            ))
        } else {
            Ok(ParsedLogin::Unprefixed { name: login })
        }
    }
}

async fn modify_owners(
    app: AppState,
    crate_name: String,
    parts: Parts,
    body: ChangeOwnersRequest,
    add: bool,
) -> AppResult<Json<ModifyResponse>> {
    let logins = body.owners;

    // Bound the number of invites processed per request to limit the cost of
    // processing them all.
    if logins.len() > 10 {
        return Err(bad_request(
            "too many invites for this request - maximum 10",
        ));
    }

    let mut conn = app.db_write().await?;
    let auth = AuthCheck::default()
        .with_endpoint_scope(EndpointScope::ChangeOwners)
        .for_crate(&crate_name)
        .check(&parts, &mut conn)
        .await?;

    let user = auth.user();

    let (msg, emails) = conn
        .transaction(async |conn| {
            let krate: Crate = Crate::by_name(&crate_name)
                .first(conn)
                .await
                .optional()?
                .ok_or_else(|| crate_not_found(&crate_name))?;

            let owners = krate.owners(conn).await?;

            match Rights::get(user, &*app.github, &owners, &app.config.token_encryption).await? {
                Rights::Full => {}
                // Yes!
                Rights::Publish => {
                    return Err(custom(
                        StatusCode::FORBIDDEN,
                        "team members don't have permission to modify owners",
                    ));
                }
                Rights::None => {
                    return Err(custom(
                        StatusCode::FORBIDDEN,
                        "only owners have permission to modify owners",
                    ));
                }
            }

            // The set of emails to send out after invite processing is complete and
            // the database transaction has committed.
            let mut emails = Vec::with_capacity(logins.len());

            let comma_sep_msg = if add {
                let mut msgs = Vec::with_capacity(logins.len());
                for login in &logins {
                    match add_owner(&app, conn, user, &krate, login, &owners).await {
                        // A user was successfully invited, and they must accept
                        // the invite, and a best-effort attempt should be made
                        // to email them the invite token for one-click
                        // acceptance.
                        Ok(NewOwnerInvite::User(invitee, token)) => {
                            msgs.push(format!(
                                "user {} has been invited to be an owner of crate {}",
                                invitee.gh_login, krate.name,
                            ));

                            if let Some(recipient) =
                                invitee.verified_email(conn).await.ok().flatten()
                            {
                                let email = EmailMessage::from_template(
                                    "owner_invite",
                                    context! {
                                        inviter => user.gh_login,
                                        domain => app.emails.domain,
                                        crate_name => krate.name,
                                        token => token.expose_secret()
                                    },
                                );

                                match email {
                                    Ok(email_msg) => emails.push((recipient, email_msg)),
                                    Err(error) => warn!(
                                        "Failed to render owner invite email template: {error}"
                                    ),
                                }
                            }
                        }

                        // A team was successfully invited. They are immediately
                        // added, and do not have an invite token.
                        Ok(NewOwnerInvite::Team(team)) => msgs.push(format!(
                            "team {} has been added as an owner of crate {}",
                            team.login, krate.name
                        )),

                        // This user has a pending invite.
                        Err(OwnerAddError::AlreadyInvited(user)) => msgs.push(format!(
                            "user {} already has a pending invitation to be an owner of crate {}",
                            user.gh_login, krate.name
                        )),

                        // An opaque error occurred.
                        Err(OwnerAddError::Diesel(e)) => return Err(e.into()),
                        Err(OwnerAddError::AppError(e)) => return Err(e),
                    }
                }
                msgs.join(",")
            } else {
                for login in &logins {
                    remove_owner(conn, &krate, login).await?;
                }
                if User::owning(&krate, conn).await?.is_empty() {
                    return Err(bad_request(
                        "cannot remove all individual owners of a crate. \
                     Team member don't have permission to modify owners, so \
                     at least one individual owner is required.",
                    ));
                }
                "owners successfully removed".to_owned()
            };

            Ok((comma_sep_msg, emails))
        })
        .await?;

    // Send the accumulated invite emails now the database state has
    // committed.
    for (recipient, email) in emails {
        if let Err(error) = app.emails.send(&recipient, email).await {
            warn!("Failed to send owner invite email to {recipient}: {error}");
        }
    }

    Ok(Json(ModifyResponse { msg, ok: true }))
}

/// Invites `login` as an owner of this crate, returning the created
/// [`NewOwnerInvite`].
async fn add_owner(
    app: &App,
    conn: &mut AsyncPgConnection,
    req_user: &User,
    krate: &Crate,
    login: &str,
    existing_owners: &[Owner],
) -> Result<NewOwnerInvite, OwnerAddError> {
    let parsed = ParsedLogin::parse(login)?;
    match parsed {
        ParsedLogin::GithubTeam { login } => {
            // Check if this team is already an owner
            let login_test =
                |owner: &Owner| owner.login().to_lowercase() == login.to_lowercase();
            if existing_owners.iter().any(login_test) {
                return Err(bad_request(format_args!("`{login}` is already an owner")).into());
            }
            let encryption = &app.config.token_encryption;
            add_team_owner(&*app.github, conn, req_user, krate, login, encryption).await
        }
        ParsedLogin::CratesIo { username } => {
            let user = User::find_by_username(conn, username)
                .await?
                .ok_or_else(|| {
                    bad_request(format_args!(
                        "could not find user with crates.io username `{username}`"
                    ))
                })?;
            check_already_owner(&user, existing_owners)?;
            invite_user_owner(app, conn, req_user, krate, user).await
        }
        ParsedLogin::GithubUser { username } => {
            let user = User::find_by_gh_login(conn, username)
                .await?
                .ok_or_else(|| {
                    bad_request(format_args!(
                        "could not find user with GitHub login `{username}`"
                    ))
                })?;
            check_already_owner(&user, existing_owners)?;
            invite_user_owner(app, conn, req_user, krate, user).await
        }
        ParsedLogin::Unprefixed { name } => {
            let user = resolve_unprefixed_user(conn, name).await?;
            check_already_owner(&user, existing_owners)?;
            invite_user_owner(app, conn, req_user, krate, user).await
        }
    }
}

/// Resolves an unprefixed login to a user, checking for ambiguity.
///
/// First looks up by `users.username`. If found, checks whether the user's
/// GitHub login matches. If they differ, returns an error asking the user
/// to disambiguate.
///
/// If not found by username, falls back to looking up by `users.gh_login`
/// for backwards compatibility.
async fn resolve_unprefixed_user(
    conn: &mut AsyncPgConnection,
    name: &str,
) -> Result<User, BoxedAppError> {
    // First, try by crates.io username
    if let Some(user) = User::find_by_username(conn, name).await? {
        // Check if the GitHub login matches the crates.io username
        if user.gh_login.to_lowercase() != name.to_lowercase() {
            return Err(disambiguation_error(name, &user));
        }
        return Ok(user);
    }

    // Fall back to GitHub login for backwards compatibility
    if let Some(user) = User::find_by_gh_login(conn, name).await? {
        // Found by GitHub login but not crates.io username — check
        // if the crates.io username differs, and if so, require disambiguation
        if user.username.to_lowercase() != name.to_lowercase() {
            return Err(disambiguation_error_from_gh(name, &user));
        }
        return Ok(user);
    }

    Err(bad_request(format_args!(
        "could not find user with login `{name}`"
    )))
}

fn disambiguation_error(name: &str, user: &User) -> BoxedAppError {
    bad_request(format!(
        "username `{name}` is possibly ambiguous.\n\
         The crates.io user `{name}` is associated with GitHub user `{}`.\n\
         To confirm this is the account you want, use one of the following:\n\n\
         cratesio:{name}\n\
         github:{}\n\n\
         If this is not the account you want, verify the crates.io username of the account you want.",
        user.gh_login, user.gh_login
    ))
}

fn disambiguation_error_from_gh(name: &str, user: &User) -> BoxedAppError {
    bad_request(format!(
        "username `{name}` is possibly ambiguous.\n\
         The GitHub user `{name}` is associated with crates.io user `{}`.\n\
         To confirm this is the account you want, use one of the following:\n\n\
         cratesio:{}\n\
         github:{name}\n\n\
         If this is not the account you want, verify the crates.io username of the account you want.",
        user.username, user.username
    ))
}

/// Check if a resolved user is already an owner.
fn check_already_owner(user: &User, owners: &[Owner]) -> Result<(), BoxedAppError> {
    if owners.iter().any(|o| o.id() == user.id) {
        return Err(bad_request(format_args!(
            "`{}` is already an owner",
            user.gh_login
        )));
    }
    Ok(())
}

async fn invite_user_owner(
    app: &App,
    conn: &mut AsyncPgConnection,
    req_user: &User,
    krate: &Crate,
    user: User,
) -> Result<NewOwnerInvite, OwnerAddError> {
    // Users are invited and must accept before being added
    let expires_at = Utc::now() + app.config.ownership_invitations_expiration;
    let invite = NewCrateOwnerInvitation {
        invited_user_id: user.id,
        invited_by_user_id: req_user.id,
        crate_id: krate.id,
        expires_at,
    };

    match invite.create(conn).await? {
        NewCrateOwnerInvitationOutcome::InviteCreated { plaintext_token } => {
            Ok(NewOwnerInvite::User(user, plaintext_token))
        }
        NewCrateOwnerInvitationOutcome::AlreadyExists => {
            Err(OwnerAddError::AlreadyInvited(Box::new(user)))
        }
    }
}

/// Removes an owner from a crate, handling prefixed logins.
async fn remove_owner(
    conn: &mut AsyncPgConnection,
    krate: &Crate,
    login: &str,
) -> Result<(), BoxedAppError> {
    let parsed = ParsedLogin::parse(login)?;
    match parsed {
        ParsedLogin::GithubTeam { login } => krate.owner_remove(conn, login).await?,
        ParsedLogin::CratesIo { username } => {
            let user = User::find_by_username(conn, username)
                .await?
                .ok_or_else(|| {
                    bad_request(format_args!(
                        "could not find user with crates.io username `{username}`"
                    ))
                })?;
            krate.owner_remove_by_user_id(conn, user.id).await?;
        }
        ParsedLogin::GithubUser { username } => {
            let user = User::find_by_gh_login(conn, username)
                .await?
                .ok_or_else(|| {
                    bad_request(format_args!(
                        "could not find user with GitHub login `{username}`"
                    ))
                })?;
            krate.owner_remove_by_user_id(conn, user.id).await?;
        }
        ParsedLogin::Unprefixed { name } => {
            resolve_and_remove_unprefixed_owner(conn, krate, name).await?;
        }
    }
    Ok(())
}

/// For unprefixed removal, only return a disambiguation error if both
/// a crates.io-username match and a GitHub-login match exist AND both
/// refer to different users who are both owners of the crate.
async fn resolve_and_remove_unprefixed_owner(
    conn: &mut AsyncPgConnection,
    krate: &Crate,
    name: &str,
) -> Result<(), BoxedAppError> {
    let by_username = User::find_by_username(conn, name).await?;
    let by_gh_login = User::find_by_gh_login(conn, name).await?;

    match (by_username, by_gh_login) {
        (Some(ci_user), Some(gh_user)) if ci_user.id != gh_user.id => {
            // Two different users — check if both are owners
            let ci_is_owner = is_user_owner_of_crate(conn, krate.id, ci_user.id).await?;
            let gh_is_owner = is_user_owner_of_crate(conn, krate.id, gh_user.id).await?;
            match (ci_is_owner, gh_is_owner) {
                (true, true) => Err(bad_request(format!(
                    "ambiguous owner name `{name}`: both crates.io user `{}` and \
                     GitHub user `{}` are owners of this crate.\n\
                     Use `cratesio:{name}` or `github:{name}` to disambiguate.",
                    ci_user.username, gh_user.gh_login
                ))),
                (true, false) => {
                    krate.owner_remove_by_user_id(conn, ci_user.id).await?;
                    Ok(())
                }
                (false, true) => {
                    krate.owner_remove_by_user_id(conn, gh_user.id).await?;
                    Ok(())
                }
                (false, false) => {
                    // Neither resolved user is an owner, but there might be
                    // another user with this login who IS an owner (e.g.
                    // duplicate accounts). Fall back to the raw SQL approach.
                    krate.owner_remove(conn, name).await?;
                    Ok(())
                }
            }
        }
        (Some(user), _) | (_, Some(user)) => {
            // Single user found (or same user found both ways).
            // Try to remove by user ID first; if that fails, fall back to
            // the raw SQL approach which can match other users with the
            // same login (e.g. old accounts with duplicate gh_login).
            match krate.owner_remove_by_user_id(conn, user.id).await {
                Ok(()) => Ok(()),
                Err(OwnerRemoveError::NotFound { .. }) => {
                    krate.owner_remove(conn, name).await?;
                    Ok(())
                }
                Err(e) => Err(e.into()),
            }
        }
        (None, None) => {
            // No user found — fall back to the existing raw SQL approach
            // which also handles team removal by login.
            krate.owner_remove(conn, name).await?;
            Ok(())
        }
    }
}

async fn is_user_owner_of_crate(
    conn: &mut AsyncPgConnection,
    crate_id: i32,
    user_id: i32,
) -> Result<bool, diesel::result::Error> {
    use crate::schema::crate_owners;
    use diesel::dsl::exists;

    diesel::select(exists(
        crate_owners::table
            .filter(crate_owners::crate_id.eq(crate_id))
            .filter(crate_owners::owner_id.eq(user_id))
            .filter(crate_owners::owner_kind.eq(OwnerKind::User as i32))
            .filter(crate_owners::deleted.eq(false)),
    ))
    .get_result(conn)
    .await
}

async fn add_team_owner(
    gh_client: &dyn GitHubClient,
    conn: &mut AsyncPgConnection,
    req_user: &User,
    krate: &Crate,
    login: &str,
    encryption: &TokenEncryption,
) -> Result<NewOwnerInvite, OwnerAddError> {
    // github:rust-lang:owners
    let mut chunks = login.split(':');

    let team_system = chunks.next().unwrap();
    if team_system != "github" {
        let error = "unknown organization handler, only 'github:org:team' is supported";
        return Err(bad_request(error).into());
    }

    // unwrap is documented above as part of the calling contract
    let org = chunks.next().unwrap();
    let team = chunks.next().ok_or_else(|| {
        let error = "missing github team argument; format is github:org:team";
        bad_request(error)
    })?;

    // Always recreate teams to get the most up-to-date GitHub ID
    let team = create_or_update_github_team(
        gh_client,
        conn,
        &login.to_lowercase(),
        org,
        team,
        req_user,
        encryption,
    )
    .await?;

    // Teams are added as owners immediately, since the above call ensures
    // the user is a team member.
    CrateOwner::builder()
        .crate_id(krate.id)
        .team_id(team.id)
        .created_by(req_user.id)
        .build()
        .insert(conn)
        .await?;

    Ok(NewOwnerInvite::Team(team))
}

/// Tries to create or update a GitHub Team. Assumes `org` and `team` are
/// correctly parsed out of the full `name`. `name` is passed as a
/// convenience to avoid rebuilding it.
pub async fn create_or_update_github_team(
    gh_client: &dyn GitHubClient,
    conn: &mut AsyncPgConnection,
    login: &str,
    org_name: &str,
    team_name: &str,
    req_user: &User,
    encryption: &TokenEncryption,
) -> AppResult<Team> {
    // GET orgs/:org/teams
    // check that `team` is the `slug` in results, and grab its data

    // "sanitization"
    fn is_allowed_char(c: char) -> bool {
        matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_')
    }

    if let Some(c) = org_name.chars().find(|c| !is_allowed_char(*c)) {
        return Err(bad_request(format_args!(
            "organization cannot contain special \
                 characters like {c}"
        )));
    }

    let token = encryption
        .decrypt(&req_user.gh_encrypted_token)
        .map_err(|err| {
            custom(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to decrypt GitHub token: {err}"),
            )
        })?;

    let auth = GitHubAuth::bearer(token);
    let team = gh_client.team_by_name(org_name, team_name, &auth).await
        .map_err(|_| {
            bad_request(format_args!(
                "could not find the github team {org_name}/{team_name}. \
                    Make sure that you have the right permissions in GitHub. \
                    See https://doc.rust-lang.org/cargo/reference/publishing.html#github-permissions"
            ))
        })?;

    let org_id = team.organization.id;
    let gh_login = &req_user.gh_login;

    let is_team_member = gh_client
        .team_membership(org_id, team.id, gh_login, &auth)
        .await?
        .is_some_and(|m| m.is_active());

    let can_add_team =
        is_team_member || is_gh_org_owner(gh_client, org_id, gh_login, &auth).await?;

    if !can_add_team {
        return Err(custom(
            StatusCode::FORBIDDEN,
            "only members of a team or organization owners can add it as an owner",
        ));
    }

    let org = gh_client.org_by_name(org_name, &auth).await?;

    NewTeam::builder()
        .login(&login.to_lowercase())
        .org_id(org_id)
        .github_id(team.id)
        .maybe_name(team.name.as_deref())
        .maybe_avatar(org.avatar_url.as_deref())
        .build()
        .create_or_update(conn)
        .await
        .map_err(Into::into)
}

async fn is_gh_org_owner(
    gh_client: &dyn GitHubClient,
    org_id: i32,
    gh_login: &str,
    auth: &GitHubAuth,
) -> Result<bool, GitHubError> {
    let membership = gh_client.org_membership(org_id, gh_login, auth).await?;
    Ok(membership.is_some_and(|m| m.is_active_admin()))
}

/// Error results from an [`add_owner()`] call.
#[derive(Debug, Error)]
enum OwnerAddError {
    #[error(transparent)]
    Diesel(#[from] diesel::result::Error),
    /// An opaque [`BoxedAppError`].
    #[error("{0}")] // AppError does not impl Error
    AppError(BoxedAppError),

    /// The requested invitee already has a pending invite.
    ///
    /// Note: Teams are always immediately added, so they cannot have a pending
    /// invite to cause this error.
    #[error("user already has pending invite")]
    AlreadyInvited(Box<User>),
}

/// A [`BoxedAppError`] does not impl [`std::error::Error`] so it needs a manual
/// [`From`] impl.
impl From<BoxedAppError> for OwnerAddError {
    fn from(value: BoxedAppError) -> Self {
        Self::AppError(value)
    }
}

impl From<OwnerRemoveError> for BoxedAppError {
    fn from(error: OwnerRemoveError) -> Self {
        match error {
            OwnerRemoveError::Diesel(error) => error.into(),
            OwnerRemoveError::NotFound { login } => {
                bad_request(format!("could not find owner with login `{login}`"))
            }
        }
    }
}
