-- dumpflash.lua  --  stream a file/MTD partition as base64 over the console.
-- Usage on device:  lua /tmp/d.lua /dev/mtdN
-- Output is framed so the host can extract exactly one block:
--   ===B64BEGIN <path>===
--   <base64 lines, 76 cols>
--   ===B64END <path> bytes=<N>===
-- Lua 5.1, byte-safe (strings hold NULs fine). No external deps.
-- Speed: a 12-bit -> 2-char lookup table (E) does most of the work, so the
-- encoder can keep a fast (e.g. 921600) console link fed rather than CPU-bind.
-- NOTE: no single quotes anywhere -- the source is uploaded inside a
-- single-quoted shell paste, so it must not contain a "'".

local path = arg[1]
local f = assert(io.open(path, "rb"))
local B = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local byte, sub, concat, floor = string.byte, string.sub, table.concat, math.floor

-- precompute: every 12-bit value -> its 2 base64 chars
local E = {}
for v = 0, 4095 do
  E[v] = sub(B, floor(v / 64) + 1, floor(v / 64) + 1) .. sub(B, (v % 64) + 1, (v % 64) + 1)
end

io.write("===B64BEGIN ", path, "===\n")

local total = 0
local carry = ""
local linelen = 0
local out = {}
local nout = 0
local function flush() if nout > 0 then io.write(concat(out, "", 1, nout)); nout = 0 end end
local function emit(s) nout = nout + 1; out[nout] = s; if nout >= 8192 then flush() end end

while true do
  local chunk = f:read(32768)
  if not chunk then break end
  total = total + #chunk
  chunk = carry .. chunk
  local n = #chunk
  local full = n - (n % 3)
  local i = 1
  while i <= full do
    local num = byte(chunk, i) * 65536 + byte(chunk, i + 1) * 256 + byte(chunk, i + 2)
    emit(E[floor(num / 4096)])
    emit(E[num % 4096])
    linelen = linelen + 4
    if linelen >= 76 then emit("\n"); linelen = 0 end
    i = i + 3
  end
  carry = sub(chunk, full + 1)
end

-- tail: 1 or 2 leftover bytes (only ever at true EOF)
local r = #carry
if r == 1 then
  local c1 = byte(carry, 1)
  local a = floor(c1 / 4)
  local b = (c1 % 4) * 16
  emit(sub(B, a + 1, a + 1) .. sub(B, b + 1, b + 1) .. "==")
  linelen = linelen + 4
elseif r == 2 then
  local num = byte(carry, 1) * 65536 + byte(carry, 2) * 256
  local a = floor(num / 262144) % 64
  local b = floor(num / 4096) % 64
  local c = floor(num / 64) % 64
  emit(sub(B, a + 1, a + 1) .. sub(B, b + 1, b + 1) .. sub(B, c + 1, c + 1) .. "=")
  linelen = linelen + 4
end
if linelen > 0 then emit("\n") end
flush()
f:close()
io.write("===B64END ", path, " bytes=", total, "===\n")
