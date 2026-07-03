module((...), package.seeall)
local hbus = require("tasks.hal.core.hbus"):instance()
local log = require("log").logger("hal.c.utils")
local session = require("tasks.harmonywebservices.core.session")
local system = require("system")
local prefMgr = require("tasks.harmonywebservices.core.preferencemanager")
local kbdManager = require("tasks.harmonyengine.core.kbdhidmanager"):instance()
local ledPath = "sys.led"

function setLedPath(paired)
  if paired then
    ledPath = "hot.1.sys.led"
  else
    ledPath = "sys.led"
  end
end

function setLedAction(arg)
  local hbusResp = hbus:command(ledPath, {action = arg})
  if not hbusResp or type(hbusResp) ~= "table" or not hbusResp.code then
    log.notice("unable to read", ledPath, arg)
    return arg
  end
end

function resetHidDeviceState()
  local response = session.getRFInfo()
  if response and response.code == 200 and response.data and response.data.Devices then
    for _, v in pairs(response.data.Devices) do
      if v.EquadID == "16420" then
        local data = {}
        data.type = "0"
        data.addr = "0"
        kbdManager:setHidDevice(data)
        break
      end
    end
  end
end
