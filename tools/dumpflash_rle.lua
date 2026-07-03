-- dumpflash_rle.lua -- dump an MTD partition, RLE-compressing 0xFF runs, then
-- base64, over the console. NOR flash is mostly erased (0xFF), so this shrinks
-- the wire traffic a lot. At 115200 the UART is the bottleneck (lua does ~1MB/s),
-- so plain per-byte work is fine.
--
-- Wire format:
--   ===RLE64BEGIN <path>===
--   <base64 of the RLE token stream, 76-col lines>
--   ===RLE64END <path> rawbytes=<N>===
--
-- RLE token stream (pre-base64), byte-oriented, unambiguous:
--   0x00 LENhi LENlo  <LEN literal bytes>      -- copy LEN bytes verbatim (LEN<=65535)
--   0x01 BYTE  C4 C3 C2 C1                     -- BYTE repeated (C4..C1 big-endian 32-bit) times
-- Literals are length-prefixed, so they may contain 0x00/0x01 freely.
-- No single quotes anywhere (uploaded via a shell paste).

local path = arg[1]
local f = assert(io.open(path, "rb"))
local B = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local byte, sub, char, concat, floor, find, rep =
  string.byte, string.sub, string.char, table.concat, math.floor, string.find, string.rep

local FF = char(255)
local RUNPAT = rep(FF, 11) .. "*"   -- a maximal run of >=11 0xFF bytes
local LITMAX = 32768

-- 12-bit -> 2 base64 chars table
local E = {}
for v = 0, 4095 do
  E[v] = sub(B, floor(v / 64) + 1, floor(v / 64) + 1) .. sub(B, (v % 64) + 1, (v % 64) + 1)
end

-- buffered console output
local out = {}; local nout = 0
local function oflush() if nout > 0 then io.write(concat(out, "", 1, nout)); nout = 0 end end
local function oput(s) nout = nout + 1; out[nout] = s; if nout >= 8192 then oflush() end end

-- streaming base64 of an arbitrary byte stream
local b64carry = ""
local linelen = 0
local function b64feed(s)
  s = b64carry .. s
  local n = #s
  local full = n - (n % 3)
  local i = 1
  while i <= full do
    local x = byte(s, i) * 65536 + byte(s, i + 1) * 256 + byte(s, i + 2)
    oput(E[floor(x / 4096)]); oput(E[x % 4096])
    linelen = linelen + 4
    if linelen >= 76 then oput("\n"); linelen = 0 end
    i = i + 3
  end
  b64carry = sub(s, full + 1)
end
local function b64finish()
  local r = #b64carry
  if r == 1 then
    local c1 = byte(b64carry, 1)
    oput(sub(B, floor(c1 / 4) + 1, floor(c1 / 4) + 1) .. sub(B, (c1 % 4) * 16 + 1, (c1 % 4) * 16 + 1) .. "==")
  elseif r == 2 then
    local x = byte(b64carry, 1) * 65536 + byte(b64carry, 2) * 256
    local a = floor(x / 262144) % 64
    local b = floor(x / 4096) % 64
    local c = floor(x / 64) % 64
    oput(sub(B, a + 1, a + 1) .. sub(B, b + 1, b + 1) .. sub(B, c + 1, c + 1) .. "=")
  end
  b64carry = ""
end

-- RLE token emitters (feed binary tokens into the base64 stream)
local function emit_literal(s)
  local L = #s
  local i = 1
  while i <= L do
    local piece = sub(s, i, i + LITMAX - 1)
    local pl = #piece
    b64feed(char(0) .. char(floor(pl / 256)) .. char(pl % 256))
    b64feed(piece)
    i = i + pl
  end
end
local function emit_run(b, count)
  b64feed(char(1) .. char(b) ..
    char(floor(count / 16777216) % 256) .. char(floor(count / 65536) % 256) ..
    char(floor(count / 256) % 256) .. char(count % 256))
end

io.write("===RLE64BEGIN ", path, "===\n")
local total = 0
local s = f:read("*a") or ""
f:close()
total = #s
local L = total
local pos = 1
while pos <= L do
  local rs, re = find(s, RUNPAT, pos)
  if not rs then
    emit_literal(sub(s, pos))
    break
  end
  if rs > pos then emit_literal(sub(s, pos, rs - 1)) end
  emit_run(255, re - rs + 1)
  pos = re + 1
end
b64finish()
if linelen > 0 then oput("\n") end
oflush()
io.write("===RLE64END ", path, " rawbytes=", total, "===\n")
