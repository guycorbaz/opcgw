# TONHE E20/A20 motorized valve — ChirpStack driver (codec) & test guide

LoRaWAN 868 MHz, **Class A**, OTAA. Single-byte open/close protocol.

Files in this folder:

| File | What it is |
|------|------------|
| `lorawan868 …通讯协议 20240417A1-中英文.xls` | Manufacturer communication protocol (source of truth) |
| `A20-LoRaWAN-868规格书.pdf` | Hardware datasheet |
| `chirpstack-codec.js` | **ChirpStack device-profile codec** (`decodeUplink` + `encodeDownlink`) |

> This codec is a ChirpStack artifact, not part of the opcgw Rust binary. It
> lets you read valve status as decoded fields and pilot the valve directly
> from the ChirpStack UI — which is how we test before opcgw's command-send
> path is wired end-to-end.

## Protocol at a glance

**Downlink — pilot the valve (1 byte):**

| fPort | Byte | Action |
|-------|------|--------|
| 10 (`0x0A`) | `0x01` | OPEN |
| 10 (`0x0A`) | `0x02` | CLOSE |
| 11 (`0x0B`) | `0x01`–`0x3C` | set status report period 1–60 min |
| 11 (`0x0B`) | `0x00` | query current period |

**Uplink — status (1 byte on fPort 10):** `0xC1` opened · `0xC2` opening ·
`0xC3` closed · `0xC4` closing · `0xC5` blocked/retrying · `0xC6`/`0xC7`
blocked-stop fault · `0xFF` unknown. **bit 4 (`0x10`) set = low battery**
(e.g. `0xD1` = opened + low battery). fPort 11 uplink = current period (minutes).

**Class A timing:** a downlink is delivered only after the valve uplinks. It
sleeps and (default) wakes every ~20 min to report + pull queued downlinks; a
button press triggers an immediate report. So a queued open/close lands on the
valve's **next wake-up**, not instantly — press the SET button to force it
during testing.

## 1. Provision the device profile

1. ChirpStack → **Device profiles → Add** (or edit the valve's profile).
2. **MAC version** / **Regional parameters**: per the datasheet (EU868).
3. **Codec** tab → Payload codec = **JavaScript functions** → paste the entire
   contents of `chirpstack-codec.js`.

## 2. Add a valve (OTAA)

Shared join credentials for all valves (per-device DevEUI is on the board):

```
AppEUI / JoinEUI = 70B3D57ED0036D57
AppKey           = B3CAD3EF63A34E256918F15862EFE150
```

Add each of the 3 devices with its own DevEUI, assign the profile above, then
**long-press SET ~2 s** on the valve to join (red LED flashes, turns off on
success).

## 3. Test piloting from the ChirpStack UI

Device → **Queue** → Enqueue downlink → **JSON** (works because the codec
provides `encodeDownlink`):

```json
{ "command": "open" }
```
```json
{ "command": "close" }
```
```json
{ "command": "set_period", "minutes": 5 }
```

Other accepted commands: `query_period`, `poll`, or a raw passthrough
`{ "fPort": 10, "bytes": [1] }`.

Then **press SET** on the valve (or wait for its next wake-up) so the Class-A
downlink is delivered. Watch **Device → Events**: you should see the decoded
uplink (`state`, `valveOpen`, `lowBattery`, …) flip to `opening`/`open` or
`closing`/`closed`.

## 4. How this maps back to opcgw

Epic E Story E-0 wired opcgw's downlink path end-to-end. opcgw stays
**model-agnostic**: instead of opcgw encoding the valve's bytes, it enqueues a
**semantic command object** (`{"command":"open"}` / `{"command":"close"}`) and
this device-profile codec's `encodeDownlink` produces the wire bytes.

Configure a command per valve with:

- `command_port = 10`
- `command_class = "valve"`
- write **`1`** to **open**, **`0`** to **close** via the OPC UA command node

opcgw maps `1 → {"command":"open"}` and `0 → {"command":"close"}`; the codec
turns those into fPort-10 bytes `0x01` / `0x02`. (The canonical OPC UA value is
`1`/`0`, **not** the raw bytes `0x01`/`0x02` — using `0` for close is exactly
why the semantic object is needed: the raw byte `0x00` is not a valid valve
command.)

> Commands **without** `command_class` keep the legacy behaviour: the OPC UA
> value is sent verbatim as a single raw payload byte. For advanced/raw use the
> codec also accepts a `{ "fPort": 10, "bytes": [1] }` passthrough.
