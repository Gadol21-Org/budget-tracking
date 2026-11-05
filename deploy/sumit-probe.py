"""One-off, read-only look at the Sumit TEST account's credit / refund documents (staging only).

Runs on the staging VM with the staging Sumit settings in ~/budget-staging/sumit.env. Makes at most
3 API calls. Prints the shape of what comes back; the credentials never leave the process, and text
values (names, e-mails, phones, addresses) are masked so only structure, ids, types, dates and
amounts show.
"""
import datetime, json, os, urllib.request

env = dict(l.rstrip("\n").split("=", 1) for l in open(os.path.expanduser("~/budget-staging/sumit.env")) if "=" in l)
base = env.get("SUMIT_BASE_URL", "https://api.sumit.co.il")
cred = {"CompanyID": int(env["SUMIT_COMPANY_ID"]), "APIKey": env["SUMIT_API_KEY"]}
CREDIT = {5: "CreditInvoice", 6: "CreditInvoiceAndReceipt", 7: "CreditReceipt", 14: "CreditDonationReceipt"}
calls = 0


def post(path, body):
    global calls
    calls += 1
    req = urllib.request.Request(base + path, json.dumps({"Credentials": cred, **body}).encode(), {"Content-Type": "application/json", "User-Agent": "budget-probe"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        print(f"HTTP {e.code} from {path}: {e.read()[:200]!r}")
        raise SystemExit(1)


def mask(v, key=""):
    if isinstance(v, dict):
        return {k: mask(x, k) for k, x in v.items()}
    if isinstance(v, list):
        return [mask(x, key) for x in v[:4]] + (["…%d more" % (len(v) - 4)] if len(v) > 4 else [])
    if isinstance(v, str):
        return v if key.lower().endswith(("id", "number", "type", "date", "status", "currency")) or key in ("Date", "Status") else ("<text>" if v else "")
    return v


today = datetime.date.today()
r = post("/accounting/documents/list/", {"DocumentTypes": [1, 2, 4, 5, 6, 7, 14], "DateFrom": (today - datetime.timedelta(days=180)).isoformat() + "T00:00:00", "DateTo": today.isoformat() + "T23:59:59", "Paging": {"StartIndex": 0, "PageSize": 100}, "IncludeDrafts": False})
print("status:", r.get("Status"), r.get("UserErrorMessage"))
docs = (r.get("Data") or {}).get("Documents") or []
print("documents:", len(docs))
by_type = {}
for d in docs:
    by_type.setdefault(d.get("Type"), []).append(d)
print("by Type:", {t: len(v) for t, v in sorted(by_type.items(), key=lambda kv: str(kv[0]))})
if docs:
    print("list item fields:", sorted(docs[0]))
for t in CREDIT:
    for d in by_type.get(t, [])[:5]:
        print("CREDIT", CREDIT[t], json.dumps(mask(d), ensure_ascii=False))
first = next((d for t in CREDIT for d in by_type.get(t, [])), None)
if first:
    det = post("/accounting/documents/getdetails/", {"DocumentID": first.get("DocumentID"), "DocumentType": first.get("Type")})
    print("CREDIT details:", json.dumps(mask(det.get("Data")), ensure_ascii=False)[:3500])
regular = next((d for t in (1, 2, 4) for d in by_type.get(t, [])), None)
if regular:
    det = post("/accounting/documents/getdetails/", {"DocumentID": regular.get("DocumentID"), "DocumentType": regular.get("Type")})
    print("REGULAR details (fields only):", sorted((det.get("Data") or {}).keys()))
print("api calls made:", calls)
