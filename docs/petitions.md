# Public petitions (abaixo-assinados) — operator guide

## 1. What the feature does

- A member writes a petition: a title, a text and a signature goal.
- Anyone can sign. A person without an account gives a name and an e-mail address.
- The browser solves a small proof-of-work puzzle (ALTCHA). This blocks bulk robots.
  No third-party service is used.
- The signature counts only after the person opens the confirmation link.
- On confirmation, the server creates the person's account. When you turn on
  provisioning, the server also creates the account at the identity provider (SSO).
- Members sign with one click.
- The author or an admin closes a petition. Only an admin hides one.

Web pages:

| Page | Purpose |
|---|---|
| `/abaixo-assinados` | List, and the form to start a petition |
| `/abaixo-assinado/?id=<uuid>` | Text, progress bar, sign form |
| `/abaixo-assinado/confirmar/` | Target of the confirmation e-mail |

API reference: the module docs in `crates/gateway/src/petitions/mod.rs`.

The raw routes of the `dsoc-initiatives` crate (`/initiatives…`) are not mounted. They
do not know the author, the closed and hidden states, or moderation.

## 2. Turn the module on

The module is off by default. Its switch is `module.initiatives`.

1. Open `/admin/modulos`.
2. Turn on **Abaixo-assinados**.

Members can now create and sign. Visitors cannot sign yet. Do step 3 for that.

## 3. Let visitors sign

### 3.1 Set the proof-of-work key

Generate a random secret and set it:

```sh
ALTCHA_HMAC_KEY=$(openssl rand -hex 32)
```

Without `ALTCHA_HMAC_KEY`, the sign form asks visitors to sign in.

### 3.2 Choose who sends the confirmation e-mail

Set `PETITIONS_EMAIL_VERIFICATION` to one of these values.

| Value | Who sends the e-mail | Use it when |
|---|---|---|
| `local` (default) | This server, through `SMTP_*` | This server has a working SMTP account |
| `sso` | The identity provider | The provider already sends mail, and this server does not |

#### Option A — `local`

1. Set `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASS` and `SMTP_FROM`.
2. Set `PETITIONS_EMAIL_VERIFICATION=local`.

The link has a random single-use token. The database keeps only its SHA-256 hash.

Provisioning is optional in this mode. When it is on, the server creates the SSO user
after the confirmation, with the address already verified. The provider then sends a
"choose your password" link.

#### Option B — `sso`

1. Do step 4 (provisioning is required in this mode).
2. Set `PETITIONS_EMAIL_VERIFICATION=sso`.

The server creates the SSO user when the visitor submits the form. The provider sends
its own verification code. The link in that e-mail comes back to
`/abaixo-assinado/confirmar/`, and the server checks the code with the provider.

Note: in this mode, the provider's e-mail template is the generic "verify your e-mail"
message. You can change that text in the provider's settings.

### 3.3 Expiry

`PETITIONS_PENDING_TTL_HOURS` (default `72`, maximum `336`) sets how long a link works.
Expired requests are removed. If this server created an SSO user for a request that
expired, and that user is still unverified, the server removes that user too.

## 4. Create accounts at the identity provider (provisioning)

Supported provider: **Zitadel** (v2 user API).

1. In Zitadel, open the organization of your users.
2. Create a service user (machine user), for example `agora-petitions`.
3. Give it the role **Org User Manager** in that organization. Do not give it
   instance-wide roles.
4. Create a personal access token for it.
5. Set these variables:

| Variable | Value | Default |
|---|---|---|
| `PETITIONS_SSO_PROVISIONING` | `zitadel` | `none` |
| `PETITIONS_SSO_TOKEN` | the access token | — |
| `PETITIONS_SSO_API` | API base URL | `SSO_ISSUER` |
| `PETITIONS_SSO_ORG_ID` | organization id of new users | `SSO_ALLOWED_ORG_ID` |
| `PETITIONS_HANDLE_CHECK_URL` | optional, see below | — |

New users get a username from their name, for example `maria_silva_4821`.

`PETITIONS_HANDLE_CHECK_URL` is a URL prefix. The server appends the username and does
a GET. Only `404` means "free". Use it when other services share the same usernames.
Example for a Mastodon server that signs in through the same provider:

```sh
PETITIONS_HANDLE_CHECK_URL=https://mastodon.example.org/api/v1/accounts/lookup?acct=
```

### Facts about Zitadel that the code relies on

- Zitadel does **not** keep e-mail addresses unique. The server always searches for the
  address before it creates a user.
- If the address already has a **verified** user, the visitor must sign in and sign
  with one click. The form says so. (The provider cannot send a code to a verified
  address without marking it unverified.)
- In `sso` mode, after the first confirmed signature the address has a verified
  account. From then on, that person signs by signing in.
- A user created without a password is active. The password-reset link lets the person
  choose one.

## 5. Security and privacy

- Rate limits per minute (in memory): 5 sign requests per client address, 2 per e-mail
  address, 20 puzzles per client address, 10 confirmations per client address.
- Limits kept in the database (they hold across restarts):
  - `PETITIONS_RESEND_COOLDOWN_SECS` (default `300`): wait before the same address gets
    another e-mail for the same petition.
  - `PETITIONS_MAX_EMAILS_PER_HOUR` (default `200`): cap of confirmation e-mails for the
    whole instance. Above it, the form asks people to try later. This protects the mail
    reputation and the identity provider from bulk sign-ups.
- The sign-up rules in `/admin/email-domains` and `/admin/ip-rules` (scope `signup`)
  also apply to public signing, because it creates accounts.
- A confirmation locks its pending row, so a link opened twice at once counts once.
- Names accept letters, spaces and `.'-` only. The name goes into e-mail, so it cannot
  carry a link or markup.
- Account erasure (LGPD) keeps the signature in the count, like a vote, but removes the
  public name and city, and deletes open requests for the address. The export lists
  the signatures. Erasure here does NOT remove the account at the identity provider;
  the operator removes it there (as for any SSO account).
- A signature is linked to an existing account only when that account already proved
  the address (level above `anonymous`). An unconfirmed password sign-up with the same
  address is never reused, and an unverified provider user is never linked.
- Petition titles cannot contain links, site addresses or e-mail addresses, because the
  title goes into the confirmation e-mail.
- A status set by an admin (closed or hidden) is moderation: the author cannot undo it.
- Each puzzle is spent by its canonical challenge, so one solution works once.
- Each puzzle solution works once (table `altcha_spent`).
- The visitor must accept a consent text. The text says that the confirmation creates
  an account, and names the identity provider account when provisioning is on.
- The signer chooses if the name shows in the public list. The city is optional.
- The e-mail of a pending signature is stored in clear text only until it is confirmed
  or expires. Then the row is deleted.
- A configuration error (for example an unknown value) turns anonymous signing off and
  is logged once at start-up. It never opens an unchecked path.

## 6. Examples

Server sends mail, no SSO:

```sh
ALTCHA_HMAC_KEY=<random>
PETITIONS_EMAIL_VERIFICATION=local
SMTP_HOST=mail.example.org
SMTP_FROM=AGORA <noreply@example.org>
```

Identity provider sends mail and owns the accounts:

```sh
ALTCHA_HMAC_KEY=<random>
PETITIONS_EMAIL_VERIFICATION=sso
PETITIONS_SSO_PROVISIONING=zitadel
PETITIONS_SSO_TOKEN=<token of the Org User Manager service user>
# PETITIONS_SSO_API and PETITIONS_SSO_ORG_ID default to SSO_ISSUER and SSO_ALLOWED_ORG_ID
```
