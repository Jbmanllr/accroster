# What Anthropic's terms allow for multiple accounts and third-party tools

Answers [issue #3](https://github.com/Jbmanllr/claude-account-manager/issues/3) for the [map](https://github.com/Jbmanllr/claude-account-manager/issues/1). Researched 2026-10-24.

**Not legal advice.** This is a reading of Anthropic's published terms, docs and statements by an engineer, not a lawyer. Anthropic can change its terms or enforce them at its discretion. Before you distribute the app, check the live pages, and ask Anthropic (the legal page points to [contact sales](https://www.anthropic.com/contact-sales)) if a feature depends on a grey answer.

## Sources and labels

Sources are primary only:

- [Consumer Terms](https://www.anthropic.com/legal/consumer-terms) (effective Oct 8, 2025, as served on 2026-10-24)
- [Usage Policy](https://www.anthropic.com/legal/aup) (effective Sept 15, 2025)
- [Commercial Terms](https://www.anthropic.com/legal/commercial-terms)
- the Claude Code docs: [Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), [Authentication](https://code.claude.com/docs/en/authentication), [Status line](https://code.claude.com/docs/en/statusline), [Commands](https://code.claude.com/docs/en/commands), [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)
- Claude help center articles
- posts by Anthropic staff and Anthropic accounts on X
- the `anthropics/claude-code` issue tracker

Each claim is labeled:

- **Terms**: contract or policy text.
- **Docs**: an official doc or help-center article.
- **Staff statement**: a public post by an Anthropic employee or an official Anthropic account. It shows intent but is not contract text.
- **Inference**: my reading.

Free, Pro and Max are governed by the Consumer Terms. Team, Enterprise and API use are governed by the Commercial Terms ([Legal and compliance § License](https://code.claude.com/docs/en/legal-and-compliance#license)).

## Verdicts

| Question | Verdict | Short reason |
|---|---|---|
| (a) One person holding several subscriptions | **Clearly allowed** | The docs show how to use several accounts. Staff say having several Max accounts is not against the terms. |
| (b) Using several accounts to extend usage | **Grey** | Nothing forbids it outright. But limits "assume ordinary, individual usage", and the Usage Policy bans bypassing product "restrictions" or using multiple accounts to circumvent guardrails. |
| (c) Third-party tools reading or reusing Claude Code OAuth credentials | **Clearly forbidden** for model traffic, a third-party sign-in, or pretending to be Claude Code. **Grey, leaning forbidden** for read-only reuse. | The docs say developers "may not collect, store, or intermediate" claude.ai credentials or session tokens. |
| (d) Calling non-public claude.ai or Anthropic endpoints, such as usage figures | **Grey, leaning forbidden.** **Clearly forbidden** through browser cookies. | No permission exists. The Consumer Terms forbid automated access without an API key "or where we otherwise explicitly permit it". A documented alternative exists. |

## (a) One person holding several subscriptions: clearly allowed

- **Docs.** Claude Code's authentication page has a section "Log in with multiple accounts". It suggests one `CLAUDE_CONFIG_DIR` per account "such as work and personal accounts". Each directory keeps its own claude.ai login. Source: https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts
- **Docs.** The help center explains how to switch between an individual account and a Team or Enterprise account on the same email. Source: https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account
- **Staff statement.** On Feb 18, 2026, Thariq Shihipar of the Claude Code team wrote that "it's not against terms of service to have multiple MAX accounts". Reselling tokens through them is. Source: https://x.com/trq212/status/2024230184287949207
- **Terms, limits on this.** Each account is personal. You "may not share your Account login information … or Account credentials" or make the Account available to anyone else (Consumer Terms §2). The Usage Policy forbids creating accounts to evade a ban and using automation to create accounts. Sources: https://www.anthropic.com/legal/consumer-terms, https://www.anthropic.com/legal/aup
- **Inference.** Team and Enterprise Profiles also fall under the employer organization's own rules. Admins can restrict logins with `forceLoginOrgUUID`. Source: https://code.claude.com/docs/en/authentication#restrict-login-to-your-organization

## (b) Using several accounts to extend usage: grey

- **Docs.** "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of Claude Code and the Agent SDK." Source: https://code.claude.com/docs/en/legal-and-compliance#acceptable-use
- **Docs.** Subscription usage "is designed to support ordinary use of native Anthropic applications". Source: https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account
- **Terms.** The Usage Policy's "Do Not Abuse Our Platform" section forbids two relevant things. One is to "create multiple accounts, to avoid detection, circumvent product guardrails". The other is to "intentionally bypass or interfere with capabilities, restrictions, or guardrails established within our products". Source: https://www.anthropic.com/legal/aup
- **Docs.** The documented answers to hitting a limit are to wait, add usage credits, or upgrade. No doc offers another account as an option. Sources: https://code.claude.com/docs/en/commands (`/rate-limit-options`), https://support.claude.com/en/articles/11145838-use-claude-code-with-your-pro-or-max-plan
- **Docs.** Anthropic reserves the right to limit usage in other ways "at our discretion". Source: https://support.claude.com/en/articles/11049741-what-is-the-max-plan
- **Staff statement.** In July 2025, Anthropic said it was acting against users "sharing and reselling accounts". Source: https://x.com/AnthropicAI/status/1949898513019466140
- **Inference.** No text forbids one person from using two plans they each pay for, for example work and personal. Whether usage limits count as "restrictions … established within our products" is not defined. A pattern that looks like rotating accounts to beat limits is where enforcement risk sits. Automated abuse filters may not tell that pattern apart from reselling.

## (c) Third-party tools reading or reusing Claude Code OAuth credentials

### Clearly forbidden

- **Docs.** OAuth authentication "is designed to support ordinary use of Claude Code and other native Anthropic applications". Source: https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use
- **Docs.** Third parties may not offer claude.ai login in their own apps, or route requests through Free, Pro or Max credentials for their users. Developers "may not collect, store, or intermediate Claude.ai credentials or session tokens". Sign-in "must complete through Anthropic's own flow". Same source.
- **Docs.** The Agent SDK overview says the same: without prior approval, third parties may not offer "claude.ai login or rate limits for their products". Source: https://code.claude.com/docs/en/agent-sdk/overview
- **Docs.** Tools that "misrepresent their identity to Anthropic's servers" or "route third-party traffic against subscription limits" are prohibited "and may be enforced against". Source: https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account

### History

The policy has changed several times. Each entry below is a primary source.

- **Jan 9, 2026, staff statement.** Anthropic tightened safeguards "against spoofing the Claude Code harness". Source: https://x.com/trq212/status/2009689809875591565
- **Feb 2026, docs (archived).** The legal page said OAuth was "intended exclusively for Claude Code and Claude.ai". It said using those tokens "in any other product, tool, or service … is not permitted". Source: https://web.archive.org/web/20260301135226/https://code.claude.com/docs/en/legal-and-compliance
- **Feb 18, 2026, staff statement.** Thariq called that change "a docs clean up". He said local experimentation with the Agent SDK and `claude -p` is fine, but businesses should use an API key. Sources: https://x.com/trq212/status/2024212378402095389, https://x.com/trq212/status/2024212380142752025
- **Apr 3, 2026, staff statement.** Boris Cherny said subscriptions "weren't built for the usage patterns of these third-party tools". Source: https://x.com/bcherny/status/2040206441756471399
- **May 13 to Oct 7, 2026, staff statement and docs.**
  - May 13: a separate credit for programmatic and third-party Agent SDK use was announced. Source: https://x.com/ClaudeDevs/status/2054610152817619388
  - June 15: the change was paused.
  - Oct 7: third-party apps can "still" use subscription limits, and Max and Team plans gain API credits.

  Sources for June 15 and Oct 7: https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan, https://support.claude.com/en/articles/17154008

### Grey, leaning forbidden

This covers a local tool that reads a Profile's token only to make non-inference calls, such as a usage figure. Using the token for model requests or offering sign-in is the forbidden case above.

- **Inference.** "Collect, store, or intermediate" is written for developers of products for others. A public open-source tool that reads Claude Code's Keychain item and stores a copy fits the words.
- **Inference.** Use by the owner alone on their own Mac is lower risk.
- **Inference.** Sending a Claude Code `User-Agent` or client identity would "misrepresent their identity". That moves the tool into the forbidden case.

## (d) Calling non-public claude.ai or Anthropic endpoints, such as usage figures

### Grey, leaning forbidden

- **Terms.** Consumer Terms §3 forbids accessing the Services "through automated or non-human means, whether through a bot, script, or otherwise". The exceptions are an Anthropic API key "or where we otherwise explicitly permit it". Source: https://www.anthropic.com/legal/consumer-terms
- **Terms.** Consumer Terms §3 also forbids two more things. One is to "crawl, scrape, or otherwise harvest data … other than as permitted". The other is to reverse engineer the Services. The Commercial Terms D.4 also forbids reverse engineering. Sources: https://www.anthropic.com/legal/consumer-terms, https://www.anthropic.com/legal/commercial-terms
- **Docs (absence).** No official page documents `api.anthropic.com/api/oauth/usage` or the claude.ai usage endpoints. The documented usage APIs are the Admin API's Usage and Cost endpoints. They are for Console organizations, not subscription limits. Source: https://platform.claude.com/docs/en/build-with-claude/usage-cost-api
- **Issue tracker.** Issue #31637 reported aggressive 429s on `/api/oauth/usage`. It was closed "not planned" and labeled `invalid`, a label that means "Issue doesn't seem to be related to Claude Code". So Anthropic does not treat the endpoint as a supported surface. Source: https://github.com/anthropics/claude-code/issues/31637. Issue #30930, a similar report, is still open with no maintainer reply: https://github.com/anthropics/claude-code/issues/30930

### Clearly forbidden

This covers the claude.ai web endpoints reached with desktop or browser cookies.

- **Docs.** The legal page forbids collecting session tokens. Source: https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use
- The map's safety bar (2) already rules this out.

### Documented alternatives

- **Docs.** Claude Code gives a configured `statusLine` command `rate_limits.five_hour` and `rate_limits.seven_day`, each with `used_percentage` and `resets_at`. These are the Session limit and the Weekly limit. They are present for Pro and Max subscribers after the first API response of a session. Source: https://code.claude.com/docs/en/statusline#rate-limit-usage
- **Docs.** `/usage` shows plan usage limits inside Claude Code. Source: https://code.claude.com/docs/en/commands
- **Docs.** claude.ai Settings > Usage shows the same limits. Source: https://support.claude.com/en/articles/9797557-usage-limit-best-practices

## Implications for the spec

These are recommendations. The owner decides.

1. **Profiles and switching: in scope.** One `CLAUDE_CONFIG_DIR` per Profile is the documented multi-account route.
   - "Adding a Profile" should start Claude Code's own login, `claude auth login` under that Profile's config directory. It must not be a custom OAuth client or a captured login, because sign-in "must complete through Anthropic's own flow".
   - The app should never mint, refresh or copy tokens itself.
2. **The standing decision "every Profile shows live limits, using one credential per Profile" carries the most terms risk.**
   - Polling `/api/oauth/usage` with each Profile's token is (c)+(d): grey, leaning forbidden. It is also unsupported and rate limited.
   - Recommended default: read limits from the documented status-line `rate_limits` feed, captured per config directory.
   - Show live values for Profiles in use. Show last-known values with an "as of" time and reset time for the others.
   - If the owner still wants direct polling, record it in an ADR as an explicit, off-by-default, at-your-own-risk option with these rules:
     - the app's own honest `User-Agent`
     - no Claude Code impersonation
     - low frequency
     - no copy of the token outside Claude Code's own Keychain item

   This changes safety bar (1) in practice: the app reads tokens but does not hold them.
3. **Framing of switching and suggestions.**
   - Present Profiles as separate subscriptions, for example work and personal. Do not present them as a way to pool, stack, extend or bypass limits, in UI copy, README or the repo description.
   - Keep the "switch to the Profile with most headroom" suggestion user-initiated and neutral.
   - Keep auto-switching out of scope; the map already does. Never move a running Session.
   - Consider making the headroom suggestion optional, or limiting it to Profiles the user has marked as interchangeable.
4. **No model traffic from the app.** The app must never send inference requests with a subscription token. It must not wrap or patch the Claude Code binary, and it must not present itself as Claude Code.
5. **Team and Enterprise Profiles.** Respect managed settings such as `forceLoginOrgUUID` and `forceLoginMethod`. Do not offer to bypass them.
6. **Naming for open source (adjacent).**
   - The Consumer Terms forbid using Anthropic's names "in connection with products or services other than the Services" without written permission. Source: https://www.anthropic.com/legal/consumer-terms
   - The legal page says third parties can't use "Claude Code or Anthropic names … as part of your own product" name. Source: https://code.claude.com/docs/en/legal-and-compliance
   - The trademark guidelines require prior approval. Source: https://www.anthropic.com/legal/trademark-guidelines
   - Before going public, consider a name that does not contain "Claude", with a plain-text "works with Claude Code" description and a no-affiliation note.

## Open questions

- Whether `claude -p "/usage"` per config directory returns limits without a model request is untested. If it does, it would be a first-party way to refresh limits for Profiles not in use.
- Whether Anthropic treats switching between one's own paid accounts at a limit as circumvention is not stated anywhere. Only a direct answer from Anthropic would settle (b).
