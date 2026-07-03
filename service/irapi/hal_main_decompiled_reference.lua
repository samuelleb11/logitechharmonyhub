local string = require("string")
local log = require("log").logger("hal.main")
local system = require("system")
local socket = require("socket")
local hbus = require("tasks.hal.core.hbus"):instance()
local MSG_HAL_TXN = "hal_transaction"
local MSG_RECV_CB = "hal_recv_cb"
system.listenMessage(MSG_HAL_TXN)
system.listenMessage(MSG_RECV_CB)
local halfiles = {}
local client
local default_address = {ip = "127.0.0.1", port = 16716}

function startTcpConnection(ip, port, timeout)
  if not ip then
    ip = default_address.ip
    port = default_address.port
  end
  client, err = socket.tcp()
  if client == nil then
    log.loggly.crashCode = "tcp_connection_failed"
    log.warning("Cannot open TCP socket:", err)
    return nil
  end
  log.notice("TCP connect to", ip, ":", port)
  if type(arg.hal) == "table" then
    default_address.ip = tostring(arg.hal[1])
    ip = default_address.ip
  end
  client:setoption("tcp-nodelay", true)
  ret, err = client:connect(ip, port)
  if ret == nil then
    log.loggly.crashCode = "tcp_connect_to_hal_failed"
    log.warning("Unable to connect to HAL daemon:", err)
    client:close()
    client = nil
    return nil
  end
  client:send(string.char(unpack({1})))
  log.notice("TCP Connection started", ip, ":", port)
  return client
end

function recvLtcp(client, callback)
  while true do
    local data, error = client:receive("*b")
    if data then
      local msg = callback(data)
      if msg then
        return msg
      end
    elseif error ~= "timeout" then
      client:close()
      log.notice("SOCKET receive failed =", error)
      if error == "closed" then
        client = nil
        return nil, error
      end
      client, error = startTcpConnection()
      if not client then
        log.notice("Cannot create TCP Connection:", error)
        return nil, error
      end
    end
  end
end

assert(startTcpConnection(default_address.ip, default_address.port))
while true do
  client:settimeout(1)
  local ret, err = client:receive("*b")
  if err == "timeout" then
    client:close()
    break
  else
    log.notice("HAL connection retry")
    system.sleep(1000)
    local ret, err = client:connect(default_address.ip, default_address.port)
  end
end
assert(startTcpConnection(default_address.ip, default_address.port))
system.broadcastMessageNoWarning("hal_connected")
while true do
  local msg, args = system.yieldMessage(result, err)
  log.debug("Message", result)
  if msg == MSG_HAL_TXN then
    if client == nil then
      startTcpConnection(default_address.ip, default_address.port)
    end
    callback, err = args[1](client)
    if err == "closed" then
      client:close()
      client = nil
    end
    if callback then
      result = nil
      if err > 0 then
        result = callback("")
      end
      if not result then
        result, err = recvLtcp(client, callback)
      end
    end
  end
end
