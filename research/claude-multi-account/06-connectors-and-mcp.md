# 06 · Connectors and MCP servers across accounts

Three different sources provide tools. They look alike in a session but are scoped differently.

| Source | Configured in | Stored | Shared across accounts |
|---|---|---|---|
| **claude.ai connectors** (remote MCP: Notion, Vercel, Google Drive…) | claude.ai → Settings → Connectors | claude.ai account (server side); per-session snapshot in the sidebar entry's `remoteMcpServersConfig` | **no** |
| **Claude Code MCP servers** | `claude mcp add` (`local`, `project`, `user` scopes) | `~/.claude.json` (`mcpServers`, `projects[<path>].mcpServers`), `<repo>/.mcp.json` | yes |
| **Desktop extensions** (local MCP bundles) | Desktop app → Extensions | `~/Library/Application Support/Claude/Claude Extensions/` | yes (allowlist keyed by org) |
| Plugin MCP servers | inside a Claude Code plugin | `~/.claude/plugins/` | yes |

## Evidence of per-account connectors

Connector names found in each account's sidebar entries:

| Account | Distinct connectors seen |
|---|---|
| A | 20 |
| B | 20 |
| C | 17 |
| D | 4 |

The sets overlap only partly; for example a calendar and a design connector appear only in B, a payments connector only in C. Each connector also needs its own OAuth sign-in per account.

## Per-tool toggles

`mcp-user-tool-toggles.json` stores which connector tools are switched off, keyed by account. Only account B had any toggles set here.

## Practical consequence

To get the same tools in every account, add each connector on claude.ai while signed into that account. Local MCP servers, extensions and plugins need nothing: they already follow you.
