# MCP endpoint

Lets an AI assistant (Claude Desktop, Claude Code, any MCP client) read and edit transactions, read the change log and list categories. Transport: MCP "Streamable HTTP", stateless: every JSON-RPC message is a `POST https://<host>/mcp` answered with one JSON body.

## Turn it on

1. Make a token: `openssl rand -hex 32`. Put it in the app's environment as `MCP_TOKEN=<token>` (production: the `BUDGET_ENV` secret, then run the Deploy workflow; staging: the `STAGING_MCP_TOKEN` repository secret). Shorter than 32 characters and the endpoint stays off. Without it the endpoint answers 404.
2. **Production only:** Cloudflare Access cannot be passed by an MCP client, so make exactly this path public, like the email endpoint: Zero Trust > Access > Applications > the budget app > add a policy **Bypass** (Everyone) for the path `/mcp`. The endpoint refuses every request without `Authorization: Bearer <MCP_TOKEN>` (compared in constant time) and reads nothing before that check. Treat the token like a password: it gives full read and write access to the household's finances. To rotate it, change the secret and redeploy.
3. Connect a client, e.g. Claude Code: `claude mcp add --transport http budget https://<host>/mcp --header "Authorization: Bearer <token>"`.

Staging (sample data only) has no Access in front of it, so there the token is the only protection.

## What it does

| Tool | |
|---|---|
| `list_transactions` | `limit` required (1-100), `cursor`, `type` income/expense, `from`/`to` (day, inclusive), `category_id` |
| `get_transaction`, `create_transaction`, `update_transaction`, `delete_transaction` | the same steps as the add/edit dialog (exchange rates, validation) |
| `list_operations_log` | `limit` required (1-100), `cursor`, `from`/`to` (UTC days), `kind`, `entity_id` |
| `list_categories`, `list_payment_methods` | everything |

Pages are keyed on `(date, id)` for transactions and on the entry id for the log, not on an offset, so something added or deleted between two calls cannot repeat or skip a row. Asking for no `limit`, or more than 100, is an error rather than a silent default.

## Things to know

- **Source is worked out, not stored.** A transaction row has no "created by" column. `source.type` is read from what points at it: a linked email row (`email`), a stored Sumit receipt record or a creation by the Sumit sync (`sumit`), `recurring_series_id` (`recurring`), the change log's creation entry (`manual` with the person's email, or `mcp`), or a Paamonim line with no creation entry (`paamonim_import`). `source.basis` says which. The change log keeps a year, so an older row with none of those is `unknown`. `created_at`/`created_by`/`last_updated_*` come from the same log and are null for such rows. An email row's `auto` (a rule added it) turns into `accepted` after the mail is closed, so that distinction is only visible for recent ones.
- **Writes have the app's side effects.** They are mirrored to Paamonim (when that is on), appear in the Sheets export, and are recorded in the change log with author `mcp` (shown in the app as "עוזר AI (MCP)"), whatever Access header the request carries.
- `list_transactions` first settles repeating charges that have come due, like the app's list; those entries are logged without an author.
- `from`/`to` of the log are UTC days (entries are stamped in UTC); transactions' are the plain dates.
- Amounts are exact integer agorot, signed (expenses negative); `amount` is the same as a decimal string. `type` follows the category's kind, or the sign when there is no category.
- Not exposed: repeating series, businesses/product tagging (read-only in the output), settings, import/export.
