# Single sign-on

ithub can sign users in with Microsoft Entra ID, Google and GitHub. Each provider is on
only when its client id is set. All three use the authorization code flow with PKCE and
a one-time `state` (plus a `nonce` for Entra and Google), held on the server for ten
minutes.

Set the address users open first. Redirect URIs are built from it:

```sh
ITHUB_PUBLIC_URL=https://ithub.example.com
```

| Provider | Redirect URI to register |
|---|---|
| Microsoft Entra ID | `https://ithub.example.com/auth/entra/callback` |
| Google | `https://ithub.example.com/auth/google/callback` |
| GitHub | `https://ithub.example.com/auth/github/callback` |

## Who gets in

1. A sign-in whose provider and subject are already linked to a user signs in as that
   user.
2. Otherwise, a **verified** email that matches a user's email (set on the Users page)
   links the identity to that user, and signs them in. That happens once; after it,
   step 1 finds them.
3. Otherwise the user sees "No ithub account yet" and is asked to have an admin add
   them. With `ITHUB_SSO_AUTO_PROVISION=viewer` (or `operator`), ithub creates the user
   instead, but **only for a provider restricted** to one tenant, Workspace domain or
   org (below). Admins are never created this way.

Users added on the Users page with an email and no password can sign in only through
SSO. The Users page lists each user's linked sign-ins.

`ITHUB_PASSWORD_LOGIN=0` hides the password form and refuses password sign-in, but only
while at least one provider is configured, so a broken SSO setup can't lock everyone
out. Remove it (or unset the providers) to get password sign-in back.

The audit log records `sso.login`, `sso.link`, `sso.provision`, `sso.denied` and
`sso.failed`.

## Microsoft Entra ID

```sh
ITHUB_ENTRA_TENANT_ID=00000000-0000-0000-0000-000000000000   # your tenant's id (GUID)
ITHUB_ENTRA_CLIENT_ID=...
ITHUB_ENTRA_CLIENT_SECRET=...
```

1. Entra admin center → **Identity → Applications → App registrations → New
   registration**.
2. Name: `ithub`. Supported account types: **Accounts in this organizational directory
   only (single tenant)**.
3. Redirect URI: platform **Web**, `https://ithub.example.com/auth/entra/callback`.
4. Copy **Application (client) ID** → `ITHUB_ENTRA_CLIENT_ID`, and **Directory (tenant)
   ID** → `ITHUB_ENTRA_TENANT_ID`. Use the GUID, not a domain name; `common`,
   `organizations` and `consumers` are refused.
5. **Certificates & secrets → New client secret**. Copy its **Value** →
   `ITHUB_ENTRA_CLIENT_SECRET`. Note the expiry date.
6. **API permissions**: the default `User.Read` delegated permission is enough. The
   scopes ithub asks for are `openid email profile`.
7. Optional, to link users by email: **Token configuration → Add optional claim → ID →
   `email` and `xms_edov`**. Entra doesn't verify the `email` claim, so ithub treats it
   as verified only when `xms_edov` is true (the domain's owner verified it). Without
   it, Entra users can't be linked by email. Use `ITHUB_SSO_AUTO_PROVISION=viewer`
   instead (sign-in is already limited to your tenant), then raise roles on the Users
   page.

ithub accepts ID tokens only from the configured tenant: the issuer must be
`https://login.microsoftonline.com/{tenant}/v2.0`, and the `tid` claim must equal the
tenant id. That counts as a restriction for auto-provisioning.

## Google

```sh
ITHUB_GOOGLE_CLIENT_ID=....apps.googleusercontent.com
ITHUB_GOOGLE_CLIENT_SECRET=...
ITHUB_GOOGLE_HD=example.com        # optional: only this Workspace domain
```

1. Google Cloud console → **APIs & Services → OAuth consent screen**. For a Workspace
   domain choose **Internal**; otherwise **External**. Scopes: `openid`, `email`,
   `profile` (the non-sensitive defaults).
2. **Credentials → Create credentials → OAuth client ID**. Type **Web application**.
3. Authorized redirect URI: `https://ithub.example.com/auth/google/callback`.
4. Copy the client id and secret into the variables above.

With `ITHUB_GOOGLE_HD`, ithub checks the ID token's `hd` claim, which Google sets only
for accounts that belong to that Workspace. It doesn't trust the email's domain, which
a personal account can share. Emails count as verified when `email_verified` is true.

## GitHub

```sh
ITHUB_GITHUB_CLIENT_ID=...
ITHUB_GITHUB_CLIENT_SECRET=...
ITHUB_GITHUB_ORG=acme              # optional: only active members of this org
```

1. GitHub → **Settings → Developer settings → OAuth Apps → New OAuth App** (or the
   org's settings, to own it there).
2. Homepage URL: `https://ithub.example.com`. Authorization callback URL:
   `https://ithub.example.com/auth/github/callback`.
3. **Generate a new client secret**. Copy the client id and secret into the variables
   above.

Scopes requested: `read:user` and `user:email`, plus `read:org` when
`ITHUB_GITHUB_ORG` is set. ithub then checks `GET /user/memberships/orgs/{org}` and
requires `state: active`. If the org restricts OAuth app access, an org owner must
approve the app (**Org settings → Third-party access**) before members can sign in.

GitHub isn't OpenID Connect, so the subject is the numeric GitHub user id, and the email
is the account's **primary, verified** address from `GET /user/emails`.

## Checking the setup

Start the server and watch the log. It names each provider that's on and why any is
off, for example:

```
Google sign-in on, limited to example.com
Entra sign-in is off: ITHUB_ENTRA_TENANT_ID must be your tenant's id, not "common"
```

The sign-in page shows a button for each provider that's on.
