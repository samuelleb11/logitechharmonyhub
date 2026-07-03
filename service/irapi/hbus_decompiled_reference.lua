module((...), package.seeall)
local system = require("works.system")
local json = require("json")
local log = require("log").logger("hal.c.hbus")
local ltcp = require("tasks.hal.core.ltcp")
local string = require("string")
local table = require("table")
local uniqueId = 1
local hbusObj
local halCommands = {
  "bthid%.",
  "ir%.",
  "rf%.",
  "sys%.",
  "wifi%.",
  "hid%."
}

local function handleCallback(packet, respData)
  if packet.type == 9 then
    if packet.isResp == true then
      if type(respData) == "table" then
        log.notice("[ type: response ] [ category : notification ]")
        table.log(respData)
      else
        log.notice("[ type: response ] [ category : notification ] [ data:", respData, "]")
      end
      return true
    else
      log.notice("[ type: request ] [ category : notification ]")
    end
  elseif packet.type == 8 then
    if packet.isResp == true then
      log.notice("[ type: response ] [ category : command ]")
      return respData
    else
      log.notice("Packet received: [ type:", packet.type, "] [ response:", packet.isResp, "]")
    end
  else
    log.notice("Packet received: [ type:", packet.type, "] [ response:", packet.isResp, "]")
  end
end

function new(self)
  local obj = {
    client,
    prevCmd,
    response
  }
  setmetatable(obj, {__index = self})
  return obj
end

function instance(self)
  if not hbusObj then
    hbusObj = new(self)
  end
  return hbusObj
end

function canForwardToHal(self, command)
  for _, v in ipairs(halCommands) do
    if string.match(command, v) then
      return true
    end
  end
end

function command(self, command, params, binaryData, timeout)
  local request = {}
  request.id = uniqueId
  request.cmd = command
  request.data = params
  request.timeout = timeout
  local ltcpRequest = json.encode(request)
  log.debug("HBus request =", ltcpRequest)
  uniqueId = uniqueId + 1
  if 65535 < uniqueId then
    uniqueId = 1
  end
  local ltcpPktSent
  log.notice("[ previous command:", self.prevCmd, "] [ new command:", command, "]")
  if self.prevCmd then
    ltcp.formCommandRequest(ltcpRequest, self.client)
    ltcpPktSent = true
  end
  local data, bdata = system.sendMessage("hal_transaction", function(client)
    self.client = client
    if command == "ir.cap" or command == "rf.pairing" then
      self.prevCmd = command
    end
    if binaryData and type(binaryData) == "table" then
      binaryData = string.char(unpack(binaryData))
    end
    if binaryData then
      ltcpRequest = ltcpRequest .. binaryData
    end
    if not ltcpPktSent then
      ltcp.formCommandRequest(ltcpRequest, client)
      self.response = ltcp.newResponse()
    else
      self.response = ltcp.resetResponse(self.response)
    end
    return function(packet)
      local data = ltcp.recvResponse(self.response, packet)
      if data then
        return handleCallback(self.response, data)
      end
    end, self.response.chunkLength
  end)
  self.prevCmd = false
  if not data then
    return
  end
  system.log.loggly.crashCode = "w_hbus_response_json_decode_error"
  local ok, jsonData, jsonLen = system.safeCall(json.decode, data)
  if not ok then
    return
  end
  local binaryData = string.sub(data, jsonLen + 1, #data)
  log.debug("hbus data received", jsonData)
  return jsonData, binaryData
end

function notify(self, stateDigest, name, param)
  local request = {}
  request.i = uniqueId
  request.s = stateDigest
  request.n = name
  request.d = param
  local ltcpRequest = json.encode(request)
  log.debug("HBus request =", ltcpRequest)
  uniqueId = uniqueId + 1
  if 65535 < uniqueId then
    uniqueId = 1
  end
  local ret = system.sendMessage("hal_transaction", function(client)
    ltcp.formNotifyRequest(ltcpRequest, client)
  end)
  return ret
end
