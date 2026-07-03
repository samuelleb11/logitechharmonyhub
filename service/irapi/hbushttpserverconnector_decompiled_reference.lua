local clientConnections = require("tasks.connectserver.core.clientconnections")
local json = require("json")
local log = require("log").logger("cs.t.hbushttpserverconnector")
local mfgData = require("tasks.mfg.core.mfgdata")
local prefMgr = require("tasks.harmonywebservices.core.preferencemanager")
local socket = require("socket")
local string = require("string")
local system = require("works.system")
local table = require("table")
local utils = require("tasks.connectserver.core.utils")
local port = 8088
local CLIENT_TIMEOUT = 60
system.sleep(600)
local httpErrors = {
  ["400"] = "Bad Request",
  ["404"] = "Not Found",
  ["405"] = "Method Not Allowed",
  ["500"] = "Internal Server Error"
}
local definedErrorCodes = {
  NoOrigin = "400.1",
  InvalidOrigin = "400.2",
  NoContentType = "400.3",
  InvalidContentType = "400.4",
  NotAllowed = "417",
  EmptyMessage = "417.1",
  InvalidMessage = "417.2",
  NoSecret = "417.3"
}

local function cleanUp(params)
  params.client:close()
  clientConnections.removeClientConnectionById(params.clientId)
end

local function handleError(params, httpCode, errorCode)
  local serverError
  if errorCode then
    serverError = json.encode({code = errorCode})
  end
  local t = {}
  t[#t + 1] = table.concat({
    "HTTP/1.1",
    httpCode,
    httpErrors[httpCode]
  }, " ")
  t[#t + 1] = "Access-Control-Allow-Headers: origin, content-type, accept"
  t[#t + 1] = "Access-Control-Allow-Method: POST, OPTIONS"
  local origin = params.request and params.request.header and params.request.header.origin
  if origin then
    t[#t + 1] = "Access-Control-Allow-Origin: " .. origin
  end
  if serverError then
    t[#t + 1] = "Content-Length: " .. #serverError + 2
    t[#t + 1] = ""
    t[#t + 1] = serverError
  else
    t[#t + 1] = "Content-Length: 0"
    t[#t + 1] = ""
  end
  t[#t + 1] = ""
  params.client:send(table.concat(t, "\r\n"))
  cleanUp(params)
end

local function trim(s)
  return (s:gsub("^%s*(.-)%s*$", "%1"))
end

local function trimFront(s)
  return (s:gsub("^%s*(.-)$", "%1"))
end

local function checkOrigin(params)
  if not params.request.header.origin then
    handleError(params, "400", definedErrorCodes.NoOrigin)
    return false
  elseif not string.find(params.request.header.origin, ".myharmony.com") then
    handleError(params, "400", definedErrorCodes.InvalidOrigin)
    return false
  end
  return true
end

local function handleWebsocketConnection(params)
  local request = params.request.header
  local path = params.request.path:gsub("^/%?", "")
  local t = {}
  for keyValue in path:gmatch("[^&]+") do
    keyValue:gsub("(.*)=(.*)", function(k, v)
      t[k] = v
    end)
  end
  if tonumber(t.hubId) ~= prefMgr.remoteId then
    log.notice("websocket connection denied due to wrong hubId", t)
    params.client:send("HTTP/1.1 401 Wrong hubId\r\n\r\n")
    params.client:close()
    return
  end
  if not prefMgr.discoveryServerUri then
    log.notice("websocket connection denied due to wrong domain", t)
    params.client:send("HTTP/1.1 401 Wrong domain\r\n\r\n")
    params.client:close()
    return
  end
  if t.domain ~= prefMgr.environment then
    log.notice("websocket connection denied due to wrong domain", t)
    params.client:send("HTTP/1.1 401 Wrong domain\r\n\r\n")
    params.client:close()
    return
  end
  table.insert(request, "")
  local upgradeRequest = table.concat(request, "\r\n")
  local protocols = {"hbus"}
  local wsHandshake = require("websocket.handshake")
  local response, protocol = wsHandshake.accept_upgrade(upgradeRequest, protocols)
  if not response then
    log.notice("websocket client handshake upgrade failed", protocol)
    params.client:close()
    return
  end
  local ok, err = params.client:send(response)
  if not ok then
    log.notice("websocket client handshake send failed", err)
    params.client:close()
    return
  end
  local WsConnection = require("tasks.connectserver.core.websocketconnection")
  local wsConnection = WsConnection:new(params.client, params.clientId)
  wsConnection:process(params.clientId, "websocket", wsConnection)
end

local function handleOptions(params)
  local os = require("os")
  local t = {
    "HTTP/1.1 200 OK",
    "Access-Control-Allow-Headers: origin, content-type, accept",
    "Access-Control-Allow-Method: POST, OPTIONS",
    "Access-Control-Allow-Origin: " .. params.request.header.origin,
    "Connection: close",
    "X-Powered-By: Express",
    "Date: " .. os.date("!%a, %d %b %Y %X GMT"),
    "Content-Type: text/plain",
    "Content-Length: 0",
    "",
    ""
  }
  params.client:send(table.concat(t, "\r\n"))
  cleanUp(params)
end

local function handlePost(params)
  local hbusMsg = params.hbusMsg
  local contentType = params.request.header["content-type"]
  if not contentType then
    return handleError(params, "400", definedErrorCodes.NoContentType)
  end
  local hubSecret, iv
  if string.match(contentType, "application/octet%-stream") then
    if type(hbusMsg) ~= "string" or #hbusMsg == 0 then
      return handleError(params, "417", definedErrorCodes.EmptyMessage)
    end
    if prefMgr.mode == 3 then
      hubSecret = prefMgr.cloudApiLogin and prefMgr.cloudApiLogin.challengeSecret
    else
      local file, err = system.io.open("/etc/nonce", "r")
      if file then
        hubSecret = file:read("*a")
        file:close()
        if hubSecret then
          hubSecret = system.md5sum(hubSecret, #hubSecret)
        end
      else
        log.notice("file read error:", err)
      end
    end
    if not hubSecret then
      return handleError(params, "417", definedErrorCodes.NoSecret)
    end
    system.log.loggly.crashCode = "debug_log_fromhex_1"
    local ok, value1 = system.safeCall(string.fromhex, hbusMsg)
    if not ok or type(value1) ~= "string" then
      local p = table.copy(params)
      p.client = nil
      local t = {
        category = "debug",
        name = "log",
        api = "debug_log_fromhex_1",
        params = p
      }
      local usageLog = require("tasks.crashlog.apihandler.usagelog")
      usageLog.postDebugEvent(t)
      return handleError(params, "417", definedErrorCodes.InvalidMessage)
    end
    hubSecret = string.fromhex(system.md5sum(hubSecret, #hubSecret))
    iv = string.fromhex(string.rep("0", 32))
    system.log.loggly.crashCode = "debug_log_decrypt"
    local ok, value2 = system.safeCall(system.aesDecrypt, value1, #hbusMsg, hubSecret, iv, 0)
    if not ok or type(value2) ~= "string" then
      local p = table.copy(params)
      p.client = nil
      local t = {
        category = "debug",
        name = "log",
        api = "debug_log_decrypt",
        params = p
      }
      local usageLog = require("tasks.crashlog.apihandler.usagelog")
      usageLog.postDebugEvent(t)
      return handleError(params, "417", definedErrorCodes.InvalidMessage)
    end
    system.log.loggly.crashCode = "debug_log_fromhex_2"
    local ok, value3 = system.safeCall(string.fromhex, value2)
    if not ok or type(value3) ~= "string" then
      local p = table.copy(params)
      p.client = nil
      local t = {
        category = "debug",
        name = "log",
        api = "debug_log_fromhex_2",
        params = p
      }
      local usageLog = require("tasks.crashlog.apihandler.usagelog")
      usageLog.postDebugEvent(t)
      return handleError(params, "417", definedErrorCodes.InvalidMessage)
    end
    hbusMsg = value3
    value1 = nil
    value2 = nil
    value3 = nil
  elseif string.match(contentType, "application/json") then
    if type(hbusMsg) ~= "string" or #hbusMsg == 0 then
      return handleError(params, "417", definedErrorCodes.EmptyMessage)
    end
  else
    return handleError(params, "400", definedErrorCodes.InvalidContentType)
  end
  system.log.loggly.crashCode = "debug_log_decode"
  local ok, decoded = system.safeCall(json.decode, hbusMsg)
  if not ok or type(decoded) ~= "table" then
    local p = table.copy(params)
    p.client = nil
    local t = {
      category = "debug",
      name = "log",
      api = "debug_log_decode",
      error = decoded,
      original = hbusMsg,
      params = p
    }
    local usageLog = require("tasks.crashlog.apihandler.usagelog")
    usageLog.postDebugEvent(t)
    return handleError(params, "417", definedErrorCodes.InvalidMessage)
  end
  hbusMsg = decoded
  decoded = nil
  if type(hbusMsg.params) ~= "table" then
    hbusMsg.params = {}
  end
  if string.match(contentType, "application/json") then
    if hbusMsg.cmd == "setup.account?provision" then
      return handleError(params, "417", definedErrorCodes.NotAllowed)
    elseif prefMgr.mode == 3 and hbusMsg.cmd ~= "setup.account?getProvisionInfo" then
      return handleError(params, "417", definedErrorCodes.NotAllowed)
    else
      log.notice("received: application/json cmd", hbusMsg.cmd)
    end
  end
  clientConnections.addClientConnection(params.clientId, "hbushttp", params.client)
  hbusMsg.params.clientId = params.clientId
  hbusMsg.params.id = hbusMsg.id
  hbusMsg.params.headers = params.request.header
  hbusMsg.transport = "hbus_http"
  local engine = require("tasks.connectserver.core.engine")
  local id, result = engine.processMessage(hbusMsg)
  if not result then
    return
  end
  local response = json.encode({
    id = id,
    code = result.data.errorCode,
    msg = result.data.errorString,
    data = utils.convertBodyToTable(result.data.body)
  })
  if result.data.binary and 0 < #result.data.binary then
    local base64 = require("base64")
    response = response .. base64.encode(result.data.binary)
  end
  if string.match(contentType, "application/octet%-stream") then
    local padLength = 16 - #response % 16
    if padLength < 16 then
      response = string.format("%s%s", response, string.rep(" ", padLength))
    end
    response = system.aesEncrypt(response, #response, hubSecret, iv, 0)
  end
  utils.sendHbusHttpPostResponse(params.client, params.request.header.origin, response, contentType)
  cleanUp(params)
end

local function getRequest(params)
  local hbusMsg = {}
  local request = {
    header = {},
    method = nil,
    path = nil,
    body = nil
  }
  while true do
    local line, err = params.client:receive("*i")
    if (request.method == "OPTIONS" or request.method == "GET") and (not line or line == "") then
      break
    end
    if err then
      handleError(params, "500", err)
      return
    end
    line = trimFront(line)
    local index = string.find(line, ":")
    if not request.method then
      request.method, request.path = string.match(line, "(%w+) (.*) HTTP/1%.1")
      if not request.method then
        cleanUp(params)
        return
      end
      request.method = string.upper(request.method)
      if request.method ~= "OPTIONS" and request.method ~= "POST" and request.method ~= "GET" then
        handleError(params, "405", request.method)
        return
      end
      local path = ""
      if request.path then
        path = string.match(request.path, "/(%w*)%??.*$")
      end
      if request.path and path == "description" then
        local obj = require("tasks.connectserver.transport.ssdpdeviceinfo")
        return obj.deviceDescription(params.client)
      end
      if not request.path or string.sub(request.path, 1, 1) ~= "/" or 0 < #path then
        handleError(params, "404", request.path)
        return
      end
      if request.method == "GET" then
        table.insert(request.header, line)
      end
    elseif index and string.sub(line, 1, 1) ~= "{" then
      if request.method == "GET" then
        table.insert(request.header, line)
      else
        local key = string.sub(line, 1, index - 1)
        local value = string.sub(line, index + 1)
        if key and value then
          key = string.lower(key)
          request.header[key] = trim(value)
        end
      end
    else
      hbusMsg[#hbusMsg + 1] = line
      local msgLen = #line
      while msgLen < tonumber(request.header["content-length"]) do
        line, err = params.client:receive("*i")
        if not line or line == "" then
          break
        end
        hbusMsg[#hbusMsg + 1] = line
        msgLen = msgLen + #line
      end
      break
    end
  end
  params.request = request
  params.hbusMsg = table.concat(hbusMsg)
  return true
end

local function clientConnection(params)
  getRequest(params)
  if not params.request then
    return
  end
  if params.request.method == "GET" then
    return handleWebsocketConnection(params)
  end
  if not checkOrigin(params) then
    return
  end
  if params.request.method == "OPTIONS" then
    return handleOptions(params)
  else
    return handlePost(params)
  end
end

local server = assert(socket.bind("0.0.0.0", port))
server:setoption("reuseaddr", true)
local ip, port = server:getsockname()
if mfgData.hasNetwork == true then
  local ip2 = system.getNetworkAttribute("ipaddr")
  ip = ip2 or ip
end
log.notice("server started", ip, ":", port)
while true do
  local client, msg = server:accept()
  if client then
    client:setoption("tcp-nodelay", true)
    if mfgData.hasNetwork then
      client:settimeout(CLIENT_TIMEOUT)
    end
    local params = {
      clientId = clientConnections.getNewClientId(),
      client = client
    }
    system.addTask("client", clientConnection, params)
  else
    log.notice("Error in creating connection", msg)
  end
end
