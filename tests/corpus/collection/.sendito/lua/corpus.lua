-- Catalog functions for the wire corpus (grpc/17-template-functions).
function order_id(prefix, n)
  return string.format("%s-%05d", prefix, tonumber(n))
end
