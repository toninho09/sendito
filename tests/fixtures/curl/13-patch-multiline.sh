curl -X PATCH \
  https://api.example.com/users/42 \
  -H 'Content-Type: application/json' \
  --data-raw '{
  "op": "replace",
  "value": "it'\''s fine"
}'
