# MCP endpoint

Lets an AI assistant (Claude Desktop, Claude Code, any MCP client) read and edit transactions, read the change log and list categories. Transport: MCP "Streamable HTTP", stateless: every JSON-RPC message is a `POST https://<host>/mcp` answered with one JSON body.

## Turn it on

The endpoint has **no login**: anyone who can reach it can read and change everything. So it is meant to be reached only from another container on the VM, never from the internet.

1. Nothing to configure: it is always on.
2. Put both containers on the shared Docker network `budget-internal`. `deploy/deploy.sh` creates it and attaches the app; add `--network budget-internal` to the other service's `docker run` (or a compose `networks:` entry with `external: true`).
3. The other service calls `http://budget-tracking:3000/mcp` (the container's own port, by container name). Do **not** use `127.0.0.1:8082` from another container: that is its own loopback.
4. **Do not add a Cloudflare Access bypass for `/mcp`.** On the public hostname it then stays behind Access like the rest of the app. A client on your own machine can reach it through `cloudflared access` or an SSH tunnel to `127.0.0.1:8082`.

Guards that remain: a request with an `Origin` header (a browser cross-site call) is refused with 403 and a body that is not `application/json` with 415, so a web page cannot drive it even from a browser that can reach it. Staging (sample data only) has no Access in front of it, so there anyone who finds the port can use it.

## What it does

| Tool | |
|---|---|
| `list_transactions` | `limit` required (1-100), `cursor`, `type` income/expense, `from`/`to` (day, inclusive), `category_id`, `detail` |
| `summarize_transactions` | counts and totals instead of rows: `from`/`to`, `type`, `category_id`, `group_by` (`category` default, `month`, `category_month`) |
| `get_transaction`, `create_transaction`, `update_transaction`, `delete_transaction` | the same steps as the add/edit dialog (exchange rates, validation); always full detail |
| `list_operations_log` | `limit` required (1-100), `cursor`, `from`/`to` (UTC days), `kind`, `entity_id` |
| `list_categories` | every category, `detail` |
| `list_payment_methods` | everything |

## Compact output (the default for the two lists)

`list_transactions` and `list_categories` answer in short text, because their size is what an assistant pays for on every step. `detail: "full"` gives the whole JSON instead (the same as `get_transaction` shows per transaction). In compact mode the reply has **no `structuredContent`** (it would only repeat the text).

```
Transactions, newest first: returned 2 of 812 matching. next_cursor: MjAyNi0xMS0wMnw5MDk5
Columns: id|date|amount in shekels (expenses negative)|category id and path|note|extras
12345|2026-11-02|-185.40|23 בית > מזון|שופרסל
12346|2026-11-02|-50.00|23 בית > מזון|ארומה|foreign 13.89 USD (estimated rate)
```

- Amounts are **shekels** with two decimals, expenses negative (create/update still take signed agorot). Category is `<id> <path>`, empty when there is none. Notes have line breaks turned into spaces and `|` into `/`.
- Extras, only when present, joined by `; `: `foreign <original amount> <currency>` (` (estimated rate)` while the day's rate is a stand-in), `business <name>` (its own tag, else its category's), `recurring`.
- Left out: agorot and currency (ILS), type (the sign says it), payment method, source and its details, who created or changed it and when, the repeating-series id, the Paamonim sync state. All of it is in `get_transaction` or `detail: "full"`.
- Paging is unchanged: pass `next_cursor` from the header; `none (no more)` ends it. An empty result is the header and the columns line only.
- `list_categories`: `id|path (parent > child)|income or expense|extras` per line, extra `business <name>` when linked. Left out: `parent_id`, the Paamonim fields, `business_required`.
- `summarize_transactions` prints a header with the count, the net total and the income and expense totals, then one line per group (`category path|count|total`, with the month first for `month` and `category_month`; months newest first, then biggest total). Transactions without a category are `(no category)`.

Pages are keyed on `(date, id)` for transactions and on the entry id for the log, not on an offset, so something added or deleted between two calls cannot repeat or skip a row. Asking for no `limit`, or more than 100, is an error rather than a silent default.

## Things to know

- **Source is worked out, not stored.** A transaction row has no "created by" column. `source.type` is read from what points at it: a linked email row (`email`), a stored Sumit receipt record or a creation by the Sumit sync (`sumit`), `recurring_series_id` (`recurring`), the change log's creation entry (`manual` with the person's email, or `mcp`), or a Paamonim line with no creation entry (`paamonim_import`). `source.basis` says which. The change log keeps a year, so an older row with none of those is `unknown`. `created_at`/`created_by`/`last_updated_*` come from the same log and are null for such rows. An email row's `auto` (a rule added it) turns into `accepted` after the mail is closed, so that distinction is only visible for recent ones.
- **Writes have the app's side effects.** They are mirrored to Paamonim (when that is on), appear in the Sheets export, and are recorded in the change log with author `mcp` (shown in the app as "עוזר AI (MCP)"), whatever Access header the request carries.
- `list_transactions` first settles repeating charges that have come due, like the app's list; those entries are logged without an author.
- `from`/`to` of the log are UTC days (entries are stamped in UTC); transactions' are the plain dates.
- Amounts are exact integer agorot, signed (expenses negative) in the full JSON and in writes; `amount` is the same as a decimal string, and compact text shows shekels. `type` follows the category's kind, or the sign when there is no category.
- Not exposed: repeating series, businesses/product tagging (read-only in the output), settings, import/export.
