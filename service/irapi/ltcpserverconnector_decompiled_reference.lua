local clientConnections = require("tasks.connectserver.core.clientconnections")
local engine = require("tasks.connectserver.core.engine")
local json = require("json")
local log = require("log").logger("cs.t.ltcpserverconnector")
local ltcp = require("tasks.hal.core.ltcp")
local mfg = require("tasks.mfg.core.mfgdata")
local socket = require("socket")
local string = require("string")
local system = require("works.system")
local table = require("works.table")
local url = require("url")
local utils = require("tasks.connectserver.core.utils")
local MAX_CLIENTS = 5
local CLIENT_TIMEOUT = 60
local server, error = socket.tcp()
if server == nil then
  log.loggly.crashCode = "ltcpserverconnector_unable_to_create_server"
  log.error("Unable to create server:", error)
  return
end
server:setoption("reuseaddr", true)
local ret, error = server:bind("127.0.0.1", 16717)
if ret == nil then
  log.loggly.crashCode = "ltcpserverconnector_unable_to_bind"
  log.error("Unable to bind:", error)
  server:close()
  return
end
ret, error = server:listen()
if ret == nil then
  log.loggly.crashCode = "ltcpserverconnector_unable_to_listen"
  log.error("Client tcp server listen failed:", error)
  server:close()
  return
end
local logfile
if arg.ltcplog then
  local filename = "ltcp.log"
  if type(arg.ltcplog) == "table" then
    filename = arg.ltcplog[1]
  end
  log.notice("Logging ltcp requests to", filename)
  logfile = system.openCacheFile(filename, "w")
end

local function handleCommandCallback(client, clientId, packet, hbusMsg, binary)
  if logfile then
    logfile:write(tostring(system:jiffies()))
    logfile:write(":")
    logfile:write(json.encode(hbusMsg))
    logfile:write("\n")
    logfile:flush()
  end
  if not hbusMsg.params and hbusMsg.data then
    hbusMsg.params = hbusMsg.data
    hbusMsg.data = nil
  end
  if type(hbusMsg.params) ~= "table" then
    hbusMsg.params = {}
  end
  if hbusMsg.cmd and (hbusMsg.cmd == "setup.account?provision" or hbusMsg.cmd == "vnd.logitech.setup/vnd.logitech.account?provision") and hbusMsg.params.provisionInfo and hbusMsg.params.provisionInfo.name and hbusMsg.params.provisionInfo.name ~= json.null then
    hbusMsg.params.provisionInfo.name = url.decode(hbusMsg.params.provisionInfo.name)
  end
  if type(hbusMsg.params.requests) == "string" then
    hbusMsg.params.requests = url.decode(hbusMsg.params.requests)
  end
  hbusMsg.params.clientId = clientId
  hbusMsg.params.binary = binary
  hbusMsg.transport = "ltcp"
  local id, result = engine.processMessage(hbusMsg)
  local jsonResp = {}
  local response = {id = id, code = 200}
  if result then
    if type(result.data.body) == "number" then
      response.data = result.data.body
    elseif result.data.body then
      response.data = utils.convertBodyToTable(result.data.body)
    else
      response.data = {}
    end
    response.data.errorCode = result.data.errorCode
    if tonumber(response.data.errorCode) ~= 200 then
      response.data.errorString = result.data.errorString
    end
    table.insert(jsonResp, json.encode(response))
    if result.data.binary then
      table.insert(jsonResp, result.data.binary)
    end
  else
    repeat
      response.data = {errorCode = "200", errorString = "OK"}
      do break end -- pseudo-goto
      response.data = {
        errorCode = "503",
        errorString = "Server Error"
      }
    until true
    table.insert(jsonResp, json.encode(response))
  end
  ltcp.formCommandResponse(table.concat(jsonResp), packet.reqId, client)
  return
end

local function connect(client, clientId)
  local response = ltcp.newResponse()
  while true do
    local packet, error = client:receive("*b")
    if not packet then
      log.notice("ltcp socket receive failed:", error)
      client:close()
      return
    end
    local data = ltcp.recvResponse(response, packet)
    if data then
      if type(data) == "table" then
        local bdata = {}
        for i = 1, #packet do
          table.insert(bdata, string.byte(packet, i, i))
        end
        log.notice("size =", #packet)
        log.notice("Received packet ", bdata)
        log.loggly.crashCode = "ltcp_junk"
        log.warning("Invalid ltcp")
        client:close()
        log.notice("Closed Socket")
        return
      end
      if response.type == 8 and response.isResp == false then
        local msg, err = json.decode(data)
        local binary = string.sub(data, err + 1)
        if 1048576 < #data then
          data = nil
          system.gc()
        end
        handleCommandCallback(client, clientId, response, msg, binary)
      end
      response = ltcp.resetResponse(response)
    end
  end
end

while true do
  local client, error = server:accept()
  if client == nil then
    log.loggly.crashCode = "ltcpserverconnector_unable_to_accept_connection"
    log.warning("Unable to accept connection:", error)
    client:close()
  else
    client:setoption("tcp-nodelay", true)
    local clientId = clientConnections.getNewClientId()
    clientConnections.addClientConnection(clientId, "ltcp", client)
    system.addTask("ltcpclient_" .. clientId, connect, client, clientId)
  end
end
server:close()
