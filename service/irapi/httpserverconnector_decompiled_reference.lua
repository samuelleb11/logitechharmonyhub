local clientConnections = require("tasks.connectserver.core.clientconnections")
local connectUtils = require("tasks.connectserver.core.utils")
local engine = require("tasks.connectserver.core.engine")
local json = require("json")
local log = require("log").logger("cs.t.httpserverconnector")
local mfgData = require("tasks.mfg.core.mfgdata")
local socket = require("socket")
local string = require("string")
local system = require("works.system")
local table = require("table")
local url = require("url")
local port = 8222
data = ""
contentLength = nil
system.sleep(600)

local function processUri(input)
  local cmd, params = string.match(input.uri, "/api/([%w./?]+)[&]*(.*)")
  if params and string.sub(params, 1, 1) == "=" then
    local c, p = string.match(cmd, "(.*)?(.*)")
    cmd = c
    params = p .. params
  end
  input.uri = nil
  if not cmd then
    return
  end
  local message = {}
  message.cmd = cmd
  message.params = connectUtils.decodeParams(params)
  if input.body then
    system.log.loggly.crashCode = "json_decode_failure_httpserverconnector"
    local ok, decoded = system.safeCall(json.decode, input.body)
    if ok and type(decoded) == "table" then
      message.params.body = decoded
    end
  end
  message.id = message.params.id
  message.transport = "http"
  message.params.clientId = "FFFFFFFF"
  params = nil
  return engine.processMessage(message)
end

local function sendResponse(client, id, result)
  local response = {}
  response.id = id
  if result and result.data then
    if not result.data.cmd or string.find(result.data.cmd, "/") then
      response.mime = result.data.cmd
      response.errorCode = result.data.errorCode
      if result.data.errorString then
        response.errorMessage = result.data.errorString
      end
      if result.data.body then
        response.result = result.data.body
      else
        response.result = json.null
      end
    else
      response.code = result.data.errorCode
      if result.data.body then
        response.data = connectUtils.convertBodyToTable(result.data.body)
      end
    end
  end
  local body = json.encode(response)
  local t = {}
  t[#t + 1] = "HTTP/1.1 200 OK"
  t[#t + 1] = "Cache-Control: no-cache"
  t[#t + 1] = "Pragma: no-cache"
  t[#t + 1] = "Access-Control-Allow-Origin: *"
  t[#t + 1] = "Content-Length: " .. #body
  t[#t + 1] = ""
  t[#t + 1] = body
  client:send(table.concat(t, "\r\n"))
  client:close()
end

local function clientConnection(client)
  local buf, err = client:receive()
  if err then
    log.notice("HTTP read error:", err)
    client:close()
    return
  end
  local length = string.match(buf, "Content%-Length%: (%d+)")
  if length then
    contentLength = length
    data = ""
    local result = {
      data = {errorCode = 200}
    }
    sendResponse(client, nil, result)
    return
  end
  if contentLength and string.find(buf, "/api/vnd.logitech") and not string.find(buf, "response%?write") then
    contentLength = nil
  end
  local request = {}
  request.type, request.data = string.match(buf, "(%w+) (.*) HTTP/1%.1")
  if contentLength and #data < tonumber(contentLength) then
    data = data .. string.sub(request.data, 6)
    if #data < tonumber(contentLength) then
      local result = {
        data = {errorCode = 200}
      }
      sendResponse(client, nil, result)
      return
    end
  end
  if contentLength then
    request.data = "/api/" .. data
    data = ""
    contentLength = nil
  end
  if not request.type then
    log.notice("HTTP invalid request", buf)
    client:close()
    return
  end
  request.header = {}
  local body = {}
  while true do
    line, err = client:receive("*i")
    if (not line or line == "") and (not request.header["Content-Length"] or not string.match(request.header["Content-Type"], "application/json")) then
      break
    end
    if err then
      if request.type == "POST" and line then
        request.post = line
        line = gsub(line, "%&", "%?")
        request.data = request.data .. "?" .. line
      end
      break
    end
    if string.sub(line, 1, 1) ~= "{" then
      key, value = string.match(line, "(.+): *(.+)")
      if key ~= nil and value ~= nil then
        request.header[key] = value
      end
    elseif string.sub(line, 1, 1) == "{" and string.match(request.header["Content-Type"], "application/json") and request.header["Content-Length"] then
      body[1] = line
      msgLen = #line
      while msgLen < tonumber(request.header["Content-Length"]) do
        line, err = client:receive("*i")
        if not line or line == "" then
          break
        end
        body[#body + 1] = line
        msgLen = msgLen + #line
      end
      break
    else
      log.notice("Missing Content-Length or Content-Type.")
    end
  end
  local input = {
    uri = request.data
  }
  if next(body) then
    input.body = table.concat(body)
  end
  request.data = nil
  local id, result = processUri(input)
  sendResponse(client, id, result)
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
  log.debug("waiting for TCP input")
  local client, msg = server:accept()
  if client then
    client:setoption("tcp-nodelay", true)
    system.addTask("client", clientConnection, client)
  elseif msg and msg ~= "timeout" then
    log.loggly.crashCode = "http_server_accept_failed"
    log.error("server:accept() failed due to", msg)
  end
end
