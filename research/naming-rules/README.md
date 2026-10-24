# What Anthropic's brand and trademark rules allow in a third-party app's name and icon

Answers [issue #27](https://github.com/Jbmanllr/claude-account-manager/issues/27) for the [map](https://github.com/Jbmanllr/claude-account-manager/issues/1). It feeds the naming decision in [#28](https://github.com/Jbmanllr/claude-account-manager/issues/28). Researched 2026-10-25. All pages below were retrieved on that date.

**Not legal advice.** This is an engineer's reading of Anthropic's published terms and docs, not a lawyer's. Trademark rights also come from law, not only from Anthropic's pages, and Anthropic decides when to enforce them. Before you publish under a name, check the live pages. If the name depends on a grey answer, ask Anthropic at marketing@anthropic.com, the address its terms give for permission.

## Sources and labels

Primary sources:

- [Anthropic Trademark Guidelines](https://www.anthropic.com/legal/trademark-guidelines) (effective Aug 1, 2024)
- [Consumer Terms](https://www.anthropic.com/legal/consumer-terms) (effective Oct 8, 2025), which bind every Free, Pro and Max user, including the app's author
- [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) (effective Jun 17, 2025)
- the Claude Code docs: [Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), [Agent SDK overview § Branding guidelines](https://code.claude.com/docs/en/agent-sdk/overview#branding-guidelines), [Marketplace reference § Reserved names](https://code.claude.com/docs/en/plugins/marketplace-reference.md#reserved-names)
- [Anthropic Software Directory Terms](https://support.claude.com/en/articles/13145338-anthropic-software-directory-terms) (effective Mar 16, 2026)
- the comparable tools' own repositories, read through the GitHub API
- posts by the people involved in a rename, on X

Each claim is labeled:

- **Terms**: contract or policy text.
- **Docs**: an official Anthropic doc.
- **Repo**: what a comparable project's repository shows.
- **Report**: a first-person post or a news report.
- **Inference**: my reading.

## Verdicts

| Use in our app | Verdict | Short reason |
|---|---|---|
| Plain-text description that says what the app works with, such as "a menu bar app for Claude Code accounts" | **Clearly allowed** | The Claude Code legal page permits accurate plain-text statements. Every comparable tool does this. |
| A disclaimer such as "Not affiliated with Anthropic. Claude is a trademark of Anthropic, PBC." | **Clearly allowed**, and recommended | It works against the one thing every Anthropic text forbids: implied endorsement. |
| "Claude Code" in the product name, a feature name or the logo | **Clearly not allowed** | Two Claude Code doc pages forbid it outright. |
| Anthropic's or Claude's logo (the starburst), or an altered version of it, in the app icon | **Clearly not allowed** without permission | The guidelines allow marks only "in materials we approve beforehand" and forbid alterations. |
| A name that sounds or looks like "Claude" without using it, such as "Clawd" | **Clearly risky** | Anthropic asked Clawdbot to rename in January 2026. |
| Implying Anthropic built, endorses or partners with the app | **Clearly not allowed** | Every source forbids it. |
| "Claude" as a word in the product name, such as "Claude account manager" | **Grey, leaning not allowed** | Consumer Terms §13 needs prior written permission to use the name "in connection with products or services other than the Services". Several popular tools do it anyway, and I found no public rename request for them. |
| "for Claude" or "for Claude Code" as a suffix in the name | **Grey** | It reads as compatibility, but the terms make no exception for it. The Agent SDK's "Powered by Claude" allowance is for SDK partners only. |
| "claude" inside the bundle id, Homebrew cask or binary name | **Grey** | No text addresses identifiers. They are visible to users and follow the product name. |
| Claude's orange colour as the icon's main colour | **Grey** | The guidelines count "brand elements" as marks. Colour alone is not addressed. |
| Showing the Claude logo inside the app to label a provider | **Grey** | CodexBar and ClaudeBar do this. Anthropic supplies no logo for that purpose without approval. |

## What Anthropic's texts say

### Trademark Guidelines: permission first, no alterations, no implied endorsement

- **Terms.** Scope: the guidelines "apply to your use of any trademarks owned by Anthropic, PBC or its affiliates". Marks "include our names, logos, brand elements, and anything we use to identify our goods and services". The page names no mark; "Claude" falls under "names". Source: https://www.anthropic.com/legal/trademark-guidelines
- **Terms.** "You may only use our trademarks as specifically permitted by us and only in materials we approve beforehand." Permission can be withdrawn "at any time". Same source.
- **Terms.** No changes to "color, font, proportion, or otherwise". Anthropic supplies the images and the size and spacing rules. Same source.
- **Terms.** You may not use the marks in a way that "implies Anthropic's sponsorship or endorsement" or that "could diminish or damage the reputation of Anthropic". Same source.
- **Terms.** Press-release use is for parties with "an existing business relationship", through marketing@anthropic.com. Same source.
- **Inference.** The page says nothing about product names, compatibility phrases ("for", "works with"), referential use or open-source projects. It reads as written for partners and press. It neither grants nor denies the plain-text references that the Claude Code docs allow.

### Consumer Terms §13: prior written permission for use with other products

- **Terms.** "You may not, without our prior written permission, use our name, logos, or other trademarks in connection with products or services other than the Services, or in any other way that implies our affiliation, endorsement, or sponsorship." Permission requests go to marketing@anthropic.com. Source: https://www.anthropic.com/legal/consumer-terms (§13, "Use of our brand")
- **Inference.** The app's author, as a Pro or Max user, is bound by this clause. Read strictly, "Claude" in a third-party product's name is a use "in connection with" another product and needs permission. The clause has no carve-out for describing compatibility. The Claude Code legal page (next section) gives that carve-out for Claude Code.

### Commercial Terms: no grant of marks

- **Terms.** The terms "do not grant either party any rights to the other's content or intellectual property, by implication or otherwise" (§F). Anthropic may name a customer publicly (§G). Nothing grants a customer any use of Anthropic's marks. Source: https://www.anthropic.com/legal/commercial-terms

### Claude Code: plain-text references only, never in a name or logo

- **Docs.** "You can accurately say, in plain text, that your product has Claude Code preinstalled or that it runs Claude Code. But you can't use the Claude Code or Anthropic names or logos as part of your own product, feature, or company name, in your own logo, or in a way that suggests Anthropic built, endorses, or is partnered with your product. Any other use of Anthropic's names or logos is governed by our Trademark Guidelines and requires our written permission." Source: https://code.claude.com/docs/en/legal-and-compliance ("Using the Claude Code name and logo")
- **Inference.** The paragraph sits under "Can customers offer Claude Code in their products?", so it is written for products that ship or run Claude Code. Our app switches the user's own Claude Code installs, so it is the nearest rule there is. It permits a description like "manages Claude Code accounts". It forbids "Claude Code" in our name, feature names or icon. It names "Claude Code or Anthropic", not "Claude" alone. "Claude" then falls under "any other use", which requires written permission.

### Agent SDK branding: "Powered by Claude" is for SDK partners

- **Docs.** "For partners integrating the Claude Agent SDK, use of Claude branding is optional." Allowed: "Claude Agent" (preferred for dropdown menus), "Claude" inside a menu already labelled "Agents", and "{YourAgentName} Powered by Claude" if you already have an agent name. Not permitted: "Claude Code" or "Claude Code Agent", and "Claude Code-branded ASCII art or visual elements that mimic Claude Code". "Your product should maintain its own branding and not appear to be Claude Code or any Anthropic product." Source: https://code.claude.com/docs/en/agent-sdk/overview#branding-guidelines
- **Inference.** Our app does not use the Agent SDK and makes no model requests (see #3), so this allowance does not cover it. It still shows the pattern Anthropic accepts: the product keeps its own name, and "Claude" appears only as a qualifier ("Powered by Claude"). It also confirms that "Claude Code" in a third-party name is out.

### Software Directory: listing grants no marks

- **Terms.** Being listed "does not ... grant you any rights to use Anthropic's name, trademarks, or intellectual property". You may not suggest "partnership with, sponsorship by, or endorsement by Anthropic" without "Anthropic's prior written approval", and you must "at all times comply with Anthropic's Trademark Guidelines". Source: https://support.claude.com/en/articles/13145338-anthropic-software-directory-terms

### Claude Code enforces names in its own plugin system

- **Docs.** Claude Code refuses plugin marketplaces named like official ones, such as `claude-code-plugins` or `anthropic-marketplace`, and "names that impersonate an official marketplace" such as `official-claude-plugins` or `claude-plugins-v2`. It also refuses respellings of reserved names. Source: https://code.claude.com/docs/en/plugins/marketplace-reference.md#reserved-names
- **Inference.** This does not govern app names. It shows that Anthropic treats "claude-" prefixed names that look official as impersonation in its own tooling.

### Registered marks

- **Report.** Anthropic, PBC has filed US trademark applications for CLAUDE (serials 97790228 and 98325070), for CLAUDE with the starburst design (98431492), and for ANTHROPIC (98267877), among others. I could not load the USPTO status pages, so registration status is unverified. Check https://tsdr.uspto.gov before relying on it. Source: search results listing Justia's mirror of USPTO records, e.g. https://trademarks.justia.com/984/31/claude-98431492.html
- **Inference.** Registration status matters little here. Anthropic asserted its marks against a sound-alike name (next section), whatever their registration status.

## Precedent: Clawdbot renamed at Anthropic's request

- **Report.** On Jan 27, 2026, Peter Steinberger posted: "Had to rename our accounts for trademark stuff". Source: https://x.com/steipete/status/2016068265391354181 (read through api.fxtwitter.com)
- **Report.** The same day: "I was forced to rename the account by Anthropic. Wasn't my decision." Source: https://x.com/steipete/status/2016079236780449975
- **Report.** The project (an open-source AI assistant with a lobster mascot, "Clawd") became Moltbot, then OpenClaw three days later. TechCrunch described the change as following "a legal challenge from Anthropic"; it had asked Anthropic for comment. Sources: https://techcrunch.com/2026/01/27/everything-you-need-to-know-about-viral-personal-ai-assistant-clawdbot-now-moltbot/, https://en.wikipedia.org/wiki/OpenClaw
- **Report.** The rename was rushed. Scammers grabbed the old X handle and GitHub name in the seconds between release and re-registration. Same sources.
- **Inference.** I found no public statement from Anthropic, so the exact demand is unknown. What is known: Anthropic acted against a popular open-source project whose name only sounded like "Claude", and the author complied rather than contest it. A rename after launch costs the name, the handles, the bundle id and users' trust.

## How comparable tools name and describe themselves

Read through the GitHub API on 2026-10-25. "Rename requests" means I searched each repository's issues and PRs for `trademark`, `rename`, `anthropic`, `brand` and `logo`, and also searched the web.

| Tool | Name and identifiers | Description | Icon | Disclaimer | Rename request from Anthropic |
|---|---|---|---|---|---|
| [ClaudeBar](https://github.com/tddworks/ClaudeBar) (Apache-2.0, ~1.6k stars) | "ClaudeBar"; bundle id `com.tddworks.claudebar`; Homebrew cask `claudebar` | "Every AI coding quota in your menu bar"; tracks Claude, Codex, Gemini and others | A llama, unrelated to Claude's marks | None found in the README | None found |
| [CodexBar](https://github.com/steipete/CodexBar) (MIT, ~23k stars) | "CodexBar" (uses OpenAI's "Codex", not "Claude"); site codexbar.app | "Show usage stats for OpenAI Codex and Claude Code, without having to login" | Its own | None found in the README | None found. Issue [#3152](https://github.com/steipete/CodexBar/issues/3152) asked about a rename because the app covers many providers, not about trademarks; closed. |
| [Claude-Usage-Tracker](https://github.com/hamed-elfayome/Claude-Usage-Tracker) (MIT, ~3.7k stars) | Display name "Claude Usage"; bundle id `HamedElfayome.Claude-Usage`; site claudetracker.com | "Native macOS menu bar app for tracking Claude AI usage limits in real-time" | An orange "C" with a pulse line, close to Claude's colour but not the starburst | "This application is not affiliated with, endorsed by, or sponsored by Anthropic PBC. Claude is a trademark of Anthropic PBC." | None found |
| [claude-swap](https://github.com/realiti4/claude-swap) (MIT, ~4k stars) | PyPI `claude-swap`; command `cswap` | "Multi-account switcher for Claude Code" | Command-line tool | None found in the README | None found |
| [ccusage](https://github.com/ccusage/ccusage) (~19k stars) | "ccusage" ("cc" for Claude Code, never spelled out); npm `ccusage` | npm: "Analyze coding (agent) CLI token usage and costs from local data" | Its own | None found in the README | None found |

- **Repo.** ClaudeBar ([#442](https://github.com/tddworks/ClaudeBar/pull/442), [#380](https://github.com/tddworks/ClaudeBar/pull/380)) and CodexBar ([#4244](https://github.com/steipete/CodexBar/pull/4244), [#4075](https://github.com/steipete/CodexBar/pull/4075)) show each provider's logo and brand colours inside the app.
- **Inference.** "Claude" in a product name is common and, as far as public records show, not enforced so far. That is tolerance, not permission: Consumer Terms §13 still requires permission, and Anthropic can act at any time, as it did with Clawdbot. The two largest tools, CodexBar and ccusage, avoid spelling out "Claude" in their names. All five use their own icon rather than Anthropic's starburst.

## Implications for the naming decision (#28)

1. Choose a distinctive name of our own that does not contain, abbreviate into a sound-alike of, or mimic "Claude" or "Anthropic". This is the only option with no grey. The working name "Claude account manager" is in the grey zone, and "Claude Code …" would be clearly out.
2. Say what the app works with in plain text, in the tagline and README: for example, "A macOS menu bar app that shows usage limits for your Claude Code accounts and switches between them."
3. Keep "claude" out of the bundle id, Homebrew cask, binary name and domain. Base them on our own name, so a forced rename would not break installs.
4. Draw an icon of our own. Do not use or adapt the starburst, and do not make Claude's orange the main colour.
5. Add the disclaimer to the README, the About window and any website: "Not affiliated with, endorsed by or sponsored by Anthropic, PBC. Claude and Claude Code are trademarks of Anthropic, PBC."
6. Do not show the Claude logo inside the app. If a provider mark is ever needed, use text.
7. If you still want "Claude" in the name, ask marketing@anthropic.com for written permission before the first public release, and keep the reply.
