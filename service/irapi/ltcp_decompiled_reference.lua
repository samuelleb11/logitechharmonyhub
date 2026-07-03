module((...), package.seeall)
local system = require("works.system")
local log = require("log").logger("hal.c.ltcp")
local bit = require("bit")
local math = require("math")
local bnot = bit.bnot
local band, bor, bxor = bit.band, bit.bor, bit.bxor
local lshift, rshift, rol = bit.lshift, bit.rshift, bit.rol
local LTCP_PACKET_SIZE = 64
local SEC_HDR_SIZE = 2
local RESP_HDR_SIZE = 4
local SERVICE_ID = 255
CHECKSUM_SUPPORT = false
local CHECKSUM_SIZE = CHECKSUM_SUPPORT and 1 or 0
fileIoCommands = {
  open = 1,
  seek = 2,
  write = 3,
  read = 4,
  flush = 5,
  devctl = 6,
  close = 7,
  command = 8,
  notify = 9,
  ping = 0
}
local ltcpErrMsg = {
  [0] = "General",
  [1] = "SequenceMismatch",
  [2] = "Busy",
  [3] = "BadVersion",
  [4] = "UnknownHandle",
  [5] = "UnknownAction",
  [6] = "AlreadyAborted",
  [7] = "NoMoreData",
  [8] = "InvalidAddress",
  [9] = "InvalidCommand",
  [10] = "BadDataLength",
  [11] = "BadRegion",
  [12] = "CheckSumMismatch",
  [13] = "TooManyFileOpen"
}

function formCommandRequest(usrData, client)
  return formLtcpPacket("command", usrData, nil, client)
end

function formCommandResponse(usrData, reqId, client)
  return formLtcpPacket("command", usrData, reqId, client)
end

function formNotifyRequest(usrData, client)
  return formLtcpPacket("notify", usrData, reqId, client)
end

function formNotifyResponse(reqId, client)
  return formLtcpPacket("notify", nil, reqId, client)
end

function formLtcpPacket(command, usrData, requestId, client)
  local MAX_PKT = 16383
  local ltcpPacket = {}
  local noPkt = 1
  if usrData then
    noPkt = noPkt + math.ceil(#usrData / MAX_PKT)
  end
  local numSecPackets = getByte(noPkt)
  local primaryPacket, reqId = formPrimaryPacket(command, requestId, numSecPackets)
  length = #primaryPacket
  copyArray(primaryPacket, 1, ltcpPacket, 1, length)
  if not usrData then
    return ltcpPacket, reqId
  end
  local pack = string.char(unpack(ltcpPacket))
  client:send(pack)
  noPkt = noPkt - 1
  local seek = 1
  while 0 < noPkt do
    pkt = {}
    pkt[#pkt + 1] = get_message_id()
    pkt[#pkt + 1] = 128
    local len
    if noPkt == 1 then
      len = #usrData % MAX_PKT
    else
      len = MAX_PKT
    end
    if 63 < len then
      local lsb, msb
      lsb = band(len, 255)
      msb = band(rshift(len, 8), 63)
      pkt[#pkt] = bor(pkt[#pkt], 64)
      pkt[#pkt] = bor(pkt[#pkt], msb)
      pkt[#pkt + 1] = lsb
    else
      pkt[#pkt] = bor(pkt[#pkt], len)
    end
    local pack = string.char(unpack(pkt))
    client:send(pack)
    local data = string.sub(usrData, seek, seek - 1 + len)
    client:send(data)
    seek = seek + len
    noPkt = noPkt - 1
  end
  return primaryReqId
end

function formPrimaryPacket(command, requestId, ...)
  local args = {
    ...
  }
  buf = {}
  buf[1] = SERVICE_ID
  buf[2] = fileIoCommands[command]
  local mid
  if not requestId then
    mid = get_message_id()
  else
    mid = bor(requestId, 128)
  end
  buf[3] = mid
  buf[4] = #args
  local index = 5
  for i = 1, #args do
    index = formParameter(args[i], buf, index)
  end
  if CHECKSUM_SUPPORT == true then
    buf[4] = bor(buf[4], 128)
    buf[index] = xorChecksum(buf, 1, #buf)
  end
  return buf, mid
end

function formParameter(data, param, index)
  param[index] = 0
  if type(data) == "string" then
    param[index] = bor(param[index], 128)
    len = string.len(data)
    for k = 1, len do
      index = index + 1
      param[index] = string.byte(data, k)
    end
    index = index + 1
    param[index] = 0
  elseif type(data) == "table" then
    len = #data
    if data.num == false then
      param[index] = bor(len + 1, param[index], 128)
      for k = 0, len do
        index = index + 1
        param[index] = data[k]
      end
    else
      param[index] = data[0]
      for k = 1, len do
        index = index + 1
        param[index] = data[k]
      end
    end
  end
  return index + 1
end

local function addChunk(self, chunk)
  self.chunk[#self.chunk + 1] = chunk
  self.chunkLength = self.chunkLength + #chunk
  self.chunkOffset = 1
end

local function readChunk(self, len)
  if len > self.chunkLength - self.chunkOffset + 1 then
    log.debug("need more data ", self.chunkLength, self.chunkOffset, len)
    return
  end
  local data = {}
  local offset = self.chunkOffset
  for i, chunk in ipairs(self.chunk) do
    if #chunk >= offset + len - 1 then
      data[#data + 1] = string.sub(chunk, offset, offset + len - 1)
      break
    end
    local frag
    if 1 < offset then
      frag = string.sub(chunk, offset)
      offset = offset - #chunk + #frag
    else
      frag = chunk
    end
    data[#data + 1] = frag
    len = len - #frag
  end
  data = table.concat(data)
  self.chunkOffset = self.chunkOffset + #data
  return data
end

local function flushChunk(self)
  while self.chunkOffset > 1 do
    local chunk = self.chunk[1]
    if #chunk < self.chunkOffset then
      self.chunkOffset = self.chunkOffset - #chunk
      self.chunkLength = self.chunkLength - #chunk
      table.remove(self.chunk, 1)
    else
      self.chunk[1] = string.sub(chunk, self.chunkOffset)
      self.chunkLength = self.chunkLength - self.chunkOffset + 1
      self.chunkOffset = 1
    end
  end
end

local function processSecondaryPacket(self)
  if self.packets == 0 then
    return
  end
  local val = readChunk(self, 2)
  if not val then
    return
  end
  local seqNum = val:byte(1)
  if not seqNum or 63 < band(seqNum, 63) then
    log.loggly.crashCode = "ltcp_invalid_sequence_number"
    log.warning("processSecondaryPacket: invalid sequence number")
    return false
  end
  local len = val:byte(2)
  if not len then
    log.loggly.crashCode = "ltcp_nil_data_size"
    log.warning("processSecondaryPacket: packet length is nil", len)
    return
  end
  if band(len, 64) == 64 then
    val = readChunk(self, 1)
    if not val then
      return
    end
    len = band(len, 63)
    len = lshift(len, 8)
    len = bor(len, val:byte(1))
  else
    len = band(len, 63)
    if len > LTCP_PACKET_SIZE - SEC_HDR_SIZE - CHECKSUM_SIZE then
      log.loggly.crashCode = "ltcp_invalid_data_size"
      log.warning("processSecondaryPacket: invalid data size", len)
      return false
    end
  end
  local data
  if CHECKSUM_SUPPORT == true then
    local data = readChunk(self, len - CHECKSUM_SIZE)
    if not data then
      return
    end
    local chksum = readChunk(self, CHECKSUM_SIZE)
    if not chksum then
      return
    end
  else
    data = readChunk(self, len)
    if not data then
      return
    end
  end
  self.data[#self.data + 1] = data
  self.packets = self.packets - 1
  return true
end

local function processPrimaryPacket(self)
  local val = readChunk(self, RESP_HDR_SIZE)
  if not val then
    return
  end
  local serviceId = val:byte(1)
  local command = val:byte(2)
  local reqId = val:byte(3)
  if serviceId ~= SERVICE_ID then
    log.loggly.crashCode = "ltcp_invalid_service_id"
    log.warning("Invalid service Id", serviceId, command, reqId)
    return false
  end
  if reqId == 255 then
    log.loggly.crashCode = "ltcp_error_message"
    log.warning("ltcp error message")
    return false
  end
  if reqId == 254 then
    log.loggly.crashCode = "ltcp_end_of_file"
    log.warning("eof")
    return false
  end
  local numParam = band(val:byte(4), 63)
  if 3 < numParam then
    log.loggly.crashCode = "ltcp_invald_param"
    log.warning("invalid param", numParam)
    return false
  end
  local params = {}
  for i = 1, numParam do
    val = readChunk(self, 1)
    if not val then
      return
    end
    len = band(val:byte(1), 63)
    if len == 0 then
      buf = {val}
      repeat
        val = readChunk(self, 1)
        if not val then
          return
        end
        buf[#buf + 1] = val
      until val == 0
      params[i] = table.concat(buf)
    else
      local data = readChunk(self, len)
      if not data then
        return
      end
      params[i] = getNumber({
        val:byte(),
        data:byte(1, #data)
      }, 1)
    end
  end
  if CHECKSUM_SUPPORT == true then
    resp = readChunk(self, CHECKSUM_SIZE)
    if not resp then
      return
    end
  end
  self.params = params
  self.type = command
  self.isResp = band(reqId, 128) == 128
  self.reqId = reqId
  self.packets = params[1] - 1
  return true
end

function newResponse()
  return {
    data = {},
    chunk = {},
    chunkLength = 0,
    chunkOffset = 1
  }
end

function resetResponse(self)
  local resp = newResponse()
  if #self.chunk > 0 then
    resp.chunk = self.chunk
    resp.chunkLength = self.chunkLength
    resp.chunkOffset = self.chunkOffset
  end
  return resp
end

function recvResponse(self, packet)
  if not packet then
    return
  end
  addChunk(self, packet)
  if not self.type then
    local ret = processPrimaryPacket(self)
    if not ret then
      if ret == false then
        log.notice("flushing...")
        flushChunk(self)
        return {}
      end
      return
    end
    flushChunk(self)
  end
  while processSecondaryPacket(self) do
    flushChunk(self)
  end
  if self.packets == 0 then
    local data = table.concat(self.data)
    self.data = nil
    return data
  end
end

local message_id = 0

function get_message_id()
  if message_id == 64 then
    message_id = 0
  end
  ret_id = message_id
  message_id = message_id + 1
  return ret_id
end

function getUintX(bytes, val)
  uintx = {num = true}
  uintx[0] = bytes
  for i = 1, bytes do
    uintx[i] = band(rshift(val, (bytes - i) * 8), 255)
  end
  return uintx
end

function getNumber(val, start)
  num = 0
  for i = 1, val[start] do
    num = bor(num, lshift(val[start + i], (val[start] - i) * 8))
  end
  return num
end

function getArray()
  byte = {num = false}
  return byte
end

function getByte(val)
  return getUintX(1, val)
end

function getWord(val)
  return getUintX(2, val)
end

function getTriple(val)
  return getUintX(3, val)
end

function getDword(val)
  return getUintX(4, val)
end

function copyArray(src, srcOff, dest, destOff, len)
  for i = 0, len - 1 do
    dest[i + destOff] = src[i + srcOff]
  end
end

function getNumPackets(length)
  local maxSecDataSize = 16383 - SEC_HDR_SIZE
  if CHECKSUM_SUPPORT == true then
    maxSecDataSize = maxSecDataSize - CHECKSUM_SIZE
  end
  local numPackets = math.ceil(length / maxSecDataSize)
  return numPackets
end

function xorChecksum(data, offset, length)
  local crc = 0
  for i = 1, #data do
    crc = bxor(crc, data[i])
  end
  return crc
end
