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

food=$(cat_id '{"name":"Food"}')
home=$(cat_id '{"name":"Home"}')
fun=$(cat_id '{"name":"Leisure"}')
car=$(cat_id '{"name":"Car"}')
work=$(cat_id '{"name":"Salary","is_income":true}')
extra=$(cat_id '{"name":"Other income","is_income":true}')

groceries=$(cat_id "{\"name\":\"Groceries\",\"parent_id\":$food}")
eating=$(cat_id "{\"name\":\"Restaurants\",\"parent_id\":$food}")
rent=$(cat_id "{\"name\":\"Rent\",\"parent_id\":$home}")
utilities=$(cat_id "{\"name\":\"Utilities\",\"parent_id\":$home}")
trips=$(cat_id "{\"name\":\"Trips\",\"parent_id\":$fun}")
movies=$(cat_id "{\"name\":\"Movies\",\"parent_id\":$fun}")
fuel=$(cat_id "{\"name\":\"Fuel\",\"parent_id\":$car}")
pay=$(cat_id "{\"name\":\"Monthly pay\",\"parent_id\":$work}")
gifts=$(cat_id "{\"name\":\"Gifts\",\"parent_id\":$extra}")

# add <months ago> <day> <shekels> <category id> <note>; negative = expense. Amounts in agorot.
add() {
  local d agorot=$(( $3 * 100 ))
  d=$(date -d "$(date +%Y-%m-01) -$1 month +$(( $2 - 1 )) day" +%F)
  [ "$d" \> "$(date +%F)" ] && return 0     # no dates in the future
  post /api/transactions "{\"date\":\"$d\",\"amount_agorot\":$agorot,\"category_id\":$4,\"note\":\"$5\"}" >/dev/null
}

for m in 5 4 3 2 1 0; do
  add $m 1  12000 "$pay" "Salary"
  add $m 2  -4200 "$rent" "Rent"
  add $m 5   -310 "$utilities" "Electricity"
  add $m 6   -180 "$groceries" "Supermarket"
  add $m 11  -420 "$groceries" "Supermarket"
  add $m 18  -265 "$groceries" "Market"
  add $m 9   -140 "$eating" "Lunch"
  add $m 21  -230 "$eating" "Dinner out"
  add $m 14  -300 "$fuel" "Fuel"
  add $m 24   -90 "$movies" "Cinema"
done
add 3 15 -1800 "$trips" "Weekend trip"
add 1 12  -950 "$trips" "Day trip"
add 4 20   400 "$gifts" "Birthday gift"
add 0 3    250 "$gifts" "Refund"

post /api/recurring "{\"start_month\":\"$(date +%Y-%m)\",\"day\":28,\"amount_agorot\":-4900,\"category_id\":$utilities,\"note\":\"Internet\"}" >/dev/null

echo "seeded sample data"
