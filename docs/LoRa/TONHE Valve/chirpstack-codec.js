// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2026] Guy Corbaz
//
// ChirpStack device-profile payload codec for the TONHE "FULL wireless shut-off
// valve" E20 / A20 (LoRaWAN 868 MHz, Class A).
//
// Source of truth: docs/LoRa/TONHE Valve/
//   "lorawan868 ...通讯协议 20240417A1-中英文.xls"  (the communication protocol)
//
// ChirpStack v4 codec contract (JavaScript):
//   - decodeUplink(input)  : input = { bytes:[], fPort, recvTime, variables }
//                            return { data:{}, warnings:[], errors:[] }
//   - encodeDownlink(input): input = { data:{}, variables }
//                            return { bytes:[], fPort, warnings:[], errors:[] }
//
// Paste this whole file into:
//   ChirpStack UI -> Device profiles -> <your profile> -> Codec
//     Payload codec = "JavaScript functions"
//
// ---------------------------------------------------------------------------
// PROTOCOL SUMMARY
//
//   Downlink (server -> valve):
//     fPort 0x0A (10):  0x01 = set valve OPEN
//                       0x02 = set valve CLOSE
//     fPort 0x0B (11):  0x01..0x3C = set status report period (1..60 minutes)
//                       0x00       = query current report period (also used as
//                                    a no-op downlink to flush server -> device)
//
//   Uplink (valve -> server):
//     fPort 0x0A (10):  status byte
//       0xC1 opened     0xC2 opening    0xC3 closed     0xC4 closing
//       0xC5 blocked, retrying (up to 3x)
//       0xC6 blocked-stop while OPENING (retry failed)
//       0xC7 blocked-stop while CLOSING (retry failed)
//       0xFF unknown position (e.g. just powered up; issue open or close)
//       bit4 (0x10) set => LOW BATTERY  (e.g. 0xC1 -> 0xD1)
//     fPort 0x0B (11):  current report period in minutes (0x01..0x3C)
//
//   Class A: the valve only receives a downlink right after it uplinks. It
//   sleeps and (by default) wakes every 20 min to report status + pull any
//   queued downlink; a button press triggers an immediate report. So a queued
//   open/close is delivered on the valve's next wake-up, not instantly.
// ---------------------------------------------------------------------------

var FPORT_VALVE = 10; // 0x0A  open/close + status
var FPORT_PERIOD = 11; // 0x0B  report period set/query

// --- uplink status byte -> meaning (base code, low-battery bit already cleared)
var STATUS = {
  0xc1: { state: "open", text: "valve has been opened", valveOpen: true, moving: false, fault: false },
  0xc2: { state: "opening", text: "valve is being opened", valveOpen: false, moving: true, fault: false },
  0xc3: { state: "closed", text: "valve has been closed", valveOpen: false, moving: false, fault: false },
  0xc4: { state: "closing", text: "valve is being closed", valveOpen: true, moving: true, fault: false },
  0xc5: { state: "blocked", text: "blocked rotation, retrying (3x)", valveOpen: null, moving: true, fault: false },
  0xc6: { state: "fault_open", text: "blocked-stop while opening, retry failed", valveOpen: null, moving: false, fault: true },
  0xc7: { state: "fault_close", text: "blocked-stop while closing, retry failed", valveOpen: null, moving: false, fault: true }
};

function decodeUplink(input) {
  var bytes = input.bytes || [];
  var fPort = input.fPort;
  var data = {};
  var warnings = [];
  var errors = [];

  if (bytes.length !== 1) {
    errors.push("expected a 1-byte payload, got " + bytes.length + " byte(s)");
    return { data: data, warnings: warnings, errors: errors };
  }
  var raw = bytes[0] & 0xff;
  data.raw = "0x" + ("0" + raw.toString(16).toUpperCase()).slice(-2);

  if (fPort === FPORT_VALVE) {
    if (raw === 0xff) {
      data.state = "unknown";
      data.statusText = "unknown position (power-up / uncertain) - issue open or close";
      data.valveOpen = null;
      data.moving = false;
      data.fault = false;
      data.lowBattery = false; // not defined for 0xFF
      return { data: data, warnings: warnings, errors: errors };
    }
    var lowBattery = (raw & 0x10) !== 0; // bit4
    var base = raw & 0xef; // clear bit4 to recover the C1..C7 base code
    var s = STATUS[base];
    if (!s) {
      errors.push("unknown status byte " + data.raw + " on fPort " + fPort);
      return { data: data, warnings: warnings, errors: errors };
    }
    data.state = s.state;
    data.statusText = s.text;
    data.valveOpen = s.valveOpen;
    data.moving = s.moving;
    data.fault = s.fault;
    data.lowBattery = lowBattery;
    data.battery = lowBattery ? "low" : "ok";
    return { data: data, warnings: warnings, errors: errors };
  }

  if (fPort === FPORT_PERIOD) {
    if (raw < 0x01 || raw > 0x3c) {
      warnings.push("report period " + raw + " outside documented 1..60 range");
    }
    data.reportPeriodMinutes = raw;
    return { data: data, warnings: warnings, errors: errors };
  }

  errors.push("unhandled uplink fPort " + fPort);
  return { data: data, warnings: warnings, errors: errors };
}

// --- downlink: accept a friendly { command, ... } object (case-insensitive),
//     or a raw { fPort, bytes } passthrough for advanced use.
function encodeDownlink(input) {
  var d = input.data || {};
  var warnings = [];
  var errors = [];

  // Raw passthrough: { "fPort": 10, "bytes": [1] }
  if (d.bytes && d.fPort !== undefined) {
    return { bytes: d.bytes, fPort: d.fPort, warnings: warnings, errors: errors };
  }

  var cmd = (d.command || d.cmd || "").toString().toLowerCase();
  switch (cmd) {
    case "open":
      return { fPort: FPORT_VALVE, bytes: [0x01], warnings: warnings, errors: errors };
    case "close":
      return { fPort: FPORT_VALVE, bytes: [0x02], warnings: warnings, errors: errors };
    case "set_period": {
      var m = parseInt(d.minutes, 10);
      if (isNaN(m) || m < 1 || m > 60) {
        errors.push("set_period requires 'minutes' in 1..60");
        return { bytes: [], fPort: FPORT_PERIOD, warnings: warnings, errors: errors };
      }
      return { fPort: FPORT_PERIOD, bytes: [m & 0xff], warnings: warnings, errors: errors };
    }
    case "query_period":
    case "poll": // 0x00 on fPort 11: query period / no-op to flush queued data
      return { fPort: FPORT_PERIOD, bytes: [0x00], warnings: warnings, errors: errors };
    default:
      errors.push(
        "unknown command '" + cmd + "'. Use one of: open, close, set_period (minutes 1-60), query_period, poll; " +
          "or pass a raw {fPort, bytes}."
      );
      return { bytes: [], fPort: FPORT_VALVE, warnings: warnings, errors: errors };
  }
}
