-- b64decode.lua <infile.b64> <outfile> : decode a base64 text file to binary.
-- Lua 5.1, byte-safe, ignores newlines/whitespace, handles = padding.
-- No single quotes (uploaded via a single-quoted shell paste).
local inf = assert(io.open(arg[1], "rb"))
local data = inf:read("*a") or ""
inf:close()
local B = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local rev = {}
for i = 1, 64 do rev[B:byte(i)] = i - 1 end
local char, floor, concat = string.char, math.floor, table.concat
local outf = assert(io.open(arg[2], "wb"))
local out, no = {}, 0
local function emit(b) no = no + 1; out[no] = char(b); if no >= 12288 then outf:write(concat(out, "", 1, no)); no = 0 end end
local acc, nb = 0, 0
for i = 1, #data do
  local v = rev[data:byte(i)]
  if v then
    acc = acc * 64 + v; nb = nb + 1
    if nb == 4 then
      emit(floor(acc / 65536) % 256); emit(floor(acc / 256) % 256); emit(acc % 256)
      acc, nb = 0, 0
    end
  end
end
if nb == 3 then
  emit(floor(acc / 1024) % 256); emit(floor(acc / 4) % 256)
elseif nb == 2 then
  emit(floor(acc / 16) % 256)
end
if no > 0 then outf:write(concat(out, "", 1, no)) end
outf:close()
