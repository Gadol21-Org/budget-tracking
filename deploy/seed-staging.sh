#!/usr/bin/env bash
# Fills an EMPTY staging app with made-up sample data through its own HTTP API.
# Usage: seed-staging.sh <base-url>     e.g. http://127.0.0.1:8083
# Does nothing if the app already has categories. Dates are relative to today.
set -euo pipefail

base="${1:?usage: seed-staging.sh <base-url>}"

if [ "$(curl -fsS "$base/api/categories" | tr -d ' \n')" != "[]" ]; then
  echo "staging already has data; not seeding"
  exit 0
fi

post() { curl -fsS -X POST -H 'Content-Type: application/json' -d "$2" "$base$1"; }
cat_id() { post /api/categories "$1"; }   # prints the new id

food=$(cat_id '{"name":"אוכל"}')
home=$(cat_id '{"name":"בית"}')
fun=$(cat_id '{"name":"פנאי"}')
car=$(cat_id '{"name":"רכב"}')
kids=$(cat_id '{"name":"ילדים"}')
work=$(cat_id '{"name":"משכורת","is_income":true}')
biz=$(cat_id '{"name":"עסקים והכנסות צד","is_income":true}')
extra=$(cat_id '{"name":"הכנסות אחרות","is_income":true}')

groceries=$(cat_id "{\"name\":\"סופרמרקט\",\"parent_id\":$food}")
eating=$(cat_id "{\"name\":\"מסעדות\",\"parent_id\":$food}")
rent=$(cat_id "{\"name\":\"שכר דירה\",\"parent_id\":$home}")
utilities=$(cat_id "{\"name\":\"חשמל ומים\",\"parent_id\":$home}")
arnona=$(cat_id "{\"name\":\"ארנונה\",\"parent_id\":$home}")
trips=$(cat_id "{\"name\":\"טיולים\",\"parent_id\":$fun}")
movies=$(cat_id "{\"name\":\"קולנוע ובילויים\",\"parent_id\":$fun}")
fuel=$(cat_id "{\"name\":\"דלק\",\"parent_id\":$car}")
carins=$(cat_id "{\"name\":\"ביטוח רכב\",\"parent_id\":$car}")
gan=$(cat_id "{\"name\":\"גן וחוגים\",\"parent_id\":$kids}")
# Several income sources: an employee salary, two small businesses and freelance work.
salary=$(cat_id "{\"name\":\"משכורת - חברת הייטק\",\"parent_id\":$work}")
spouse=$(cat_id "{\"name\":\"משכורת - בן/בת זוג\",\"parent_id\":$work}")
shop=$(cat_id "{\"name\":\"עסק - חנות אונליין\",\"parent_id\":$biz}")
studio=$(cat_id "{\"name\":\"עסק - סטודיו לצילום\",\"parent_id\":$biz}")
freelance=$(cat_id "{\"name\":\"פרילנס - ייעוץ\",\"parent_id\":$biz}")
gifts=$(cat_id "{\"name\":\"מתנות והחזרים\",\"parent_id\":$extra}")

# add <months ago> <day> <shekels> <category id> <note>; negative = expense. Amounts in agorot.
add() {
  local d agorot=$(( $3 * 100 ))
  d=$(date -d "$(date +%Y-%m-01) -$1 month +$(( $2 - 1 )) day" +%F)
  [ "$d" \> "$(date +%F)" ] && return 0     # no dates in the future
  post /api/transactions "{\"date\":\"$d\",\"amount_agorot\":$agorot,\"category_id\":$4,\"note\":\"$5\"}" >/dev/null
}

for m in 5 4 3 2 1 0; do
  add $m 1  14500 "$salary" "משכורת"
  add $m 10  8200 "$spouse" "משכורת"
  add $m 2  -5200 "$rent" "שכר דירה"
  add $m 5   -410 "$utilities" "חשמל"
  add $m 8   -320 "$utilities" "מים וגז"
  add $m 15  -780 "$arnona" "ארנונה"
  add $m 6   -380 "$groceries" "שופרסל"
  add $m 11  -520 "$groceries" "רמי לוי"
  add $m 18  -295 "$groceries" "שוק"
  add $m 9   -140 "$eating" "ארוחת צהריים"
  add $m 21  -330 "$eating" "ארוחת ערב במסעדה"
  add $m 14  -350 "$fuel" "תדלוק"
  add $m 3  -2100 "$gan" "גן וחוגים"
  add $m 24  -110 "$movies" "קולנוע"
  add $m 20  3200 "$studio" "צילומי משפחה ואירועים"
done
# The online shop and consulting income vary from month to month.
add 5 12 4800 "$shop" "מכירות החודש"; add 4 12 6100 "$shop" "מכירות החודש"
add 3 12 3900 "$shop" "מכירות החודש"; add 2 12 7300 "$shop" "מכירות החודש"
add 1 12 5400 "$shop" "מכירות החודש"; add 0 12 4600 "$shop" "מכירות החודש"
add 4 25 5000 "$freelance" "פרויקט ייעוץ"; add 2 25 7500 "$freelance" "פרויקט ייעוץ"
add 0 4  3000 "$freelance" "ריטיינר"
add 3 15 -1800 "$trips" "סוף שבוע בצפון"
add 1 12  -950 "$trips" "טיול ליום אחד"
add 2 7  -2400 "$carins" "ביטוח רכב"
add 4 20   400 "$gifts" "מתנת יום הולדת"
add 0 3    250 "$gifts" "החזר כספי"

post /api/recurring "{\"start_month\":\"$(date +%Y-%m)\",\"day\":28,\"amount_agorot\":-4900,\"category_id\":$utilities,\"note\":\"אינטרנט\"}" >/dev/null

echo "seeded sample data"
