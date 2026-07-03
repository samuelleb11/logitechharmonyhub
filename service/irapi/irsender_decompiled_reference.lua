local hbus = require("tasks.hal.core.hbus"):instance()
local prefMgr = require("tasks.harmonywebservices.core.preferencemanager")
local system = require("system")
local string = require("string")
local log = require("log").logger("he.c.irsender")
local halUtils = require("tasks.hal.core.utils")
module((...), package.seeall)
local IDLE = 0
local RUNNING = 1
local irSenderObj

function instance(self)
  if not irSenderObj then
    irSenderObj = self:new()
    irSenderObj:setIrPath(false)
  end
  return irSenderObj
end

function new(self)
  local obj = {
    path,
    currentPorts,
    irState = IDLE
  }
  setmetatable(obj, self)
  self.__index = self
  return obj
end

function setIrPath(self, isPaired)
  if isPaired then
    self.path = "/rf/hot/1/ir/ir_send"
  else
    self.path = "/ir/ir_send"
  end
  return
end

function startIrCommand(self, data, keyLatency, ports)
  log.debug("Send IrPacket Command to ports:", ports)
  local path = self.path
  if ports == 0 then
    if prefMgr.mode == 3 then
      path = "/rf/hot/1/ir/ir_send"
    else
      path = "/ir/ir_send"
    end
  end
  self.currentPorts = ports
  if ports ~= 0 then
    halUtils.setLedAction("fire_command")
  end
  local response = hbus:command(path, {
    enable = true,
    keyLatency = keyLatency,
    ports = ports
  }, data)
  if response then
    if response.code == 500 then
      return nil, response.code, "RF timed out"
    elseif response.code == 503 then
      return nil, response.code, "RF link lost"
    end
  end
  self.irState = RUNNING
  return true, 200, "OK"
end

function startIr(self, data, keyLatency, ports)
  log.notice("Send IrPacket")
  if self.irState == RUNNING then
    self:cancelIr()
  end
  return self:startIrCommand(data, keyLatency, ports)
end

function sendIrHeartBeat(self)
  if self.irState == IDLE then
    return nil
  end
  log.notice("Send Heart Beat")
  local path = self.path
  if self.currentPorts == 0 then
    if prefMgr.mode == 3 then
      path = "/rf/hot/1/ir/ir_send"
    else
      path = "/ir/ir_send"
    end
  else
    halUtils.setLedAction("fire_command")
  end
  hbus:command(path, {enable = true})
  return self.irState
end

function cancelIr(self)
  log.notice("Cancel IrSend")
  if self.irState ~= RUNNING then
    return true, 200, "OK"
  end
  self.irState = IDLE
  local path = self.path
  if self.currentPorts == 0 then
    if prefMgr.mode == 3 then
      path = "/rf/hot/1/ir/ir_send"
    else
      path = "/ir/ir_send"
    end
  end
  hbus:command(path, {enable = false})
  return true, 200, "OK"
end
