# TONHE E20/A20 motorized valve — ChirpStack driver (codec) & test guide

LoRaWAN 868 MHz, **Class A**, OTAA. Single-byte open/close protocol.

Files in this folder:

| File | What it is |
|------|------------|
| `lorawan868 …通讯协议 20240417A1-中英文.xls` | Manufacturer communication protocol (source of truth) |
| `A20-LoRaWAN-868规格书.pdf` | Hardware datasheet |
| `tonhe-e20-valve-codec.js` | **ChirpStack device-profile codec** (`decodeUplink` + `encodeDownlink`) |

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

**Decoded fields.** opcgw reads ChirpStack's numeric `metrics` map only (it
ignores the `states` map), so the machine-readable fields are emitted as
**integers** — give each one kind = **Gauge** in the device profile's
Measurements:

| Field | Type | Meaning |
|-------|------|---------|
| `valveStatusCode` | int | raw status byte, lossless — `193` open, `194` opening, `195` closed, `196` closing, `197` blocked, `198`/`199` fault, `255` unknown; low-battery variant = base + 16 |
| `valvePosition` | int | `1` open, `0` closed, `-1` indeterminate |
| `moving` | int | `0` / `1` |
| `fault` | int | `0` / `1` |
| `lowBattery` | int | `0` / `1` (status-byte bit 4) |
| `state`, `statusText` | string | human-readable; **ChirpStack UI only** — invisible to opcgw |

**Class A timing:** a downlink is delivered only after the valve uplinks. It
sleeps and (default) wakes every ~20 min to report + pull queued downlinks; a
button press triggers an immediate report. So a queued open/close lands on the
valve's **next wake-up**, not instantly — press the SET button to force it
during testing.

## 1. Provision the device profile

1. ChirpStack → **Device profiles → Add** (or edit the valve's profile).
2. **MAC version** / **Regional parameters**: per the datasheet (EU868).
3. **Codec** tab → Payload codec = **JavaScript functions** → paste the entire
   contents of `tonhe-e20-valve-codec.js`.

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

## 5. Reading valve status back into opcgw

> **⚠️ Do not surface valve state through the metrics-poll path.** opcgw's
> current poller calls ChirpStack `GetMetrics`, which **time-aggregates** every
> uplink in a bucket (Gauge → average, Absolute → sum, Counter → delta). A valve
> position is **discrete** — it has no meaningful average or sum — so a
> close-cycle that emits `closing` then `closed` seconds apart aggregates to
> nonsense (`valveStatusCode = 196 + 195 = 391`, `valvePosition = 1.5`). This is
> structural: **no measurement kind makes an enumerated/instantaneous state
> survive aggregation.** It happens to *look* right only when exactly one uplink
> falls in a bucket (valve idle).
>
> The same flaw applies in principle to **every** point — a SCADA tag is a
> *last-known value + source timestamp + quality*, and aggregation/trending is
> the **SCADA's** job, not the gateway's. Analog sensors merely hide it (a short
> average ≈ the last reading).
>
> **Correct path — Story E-1:** ingest the uplink **event stream**
> (`StreamDeviceEvents`) and store the **last decoded value** of each field with
> the device's own timestamp — no aggregation.

**✅ Implemented for valves (Story E-1a).** opcgw now runs an uplink
event-ingestion task that consumes `InternalService.StreamDeviceEvents` for
**valve-class devices** (any device whose command has `command_class = "valve"`)
and writes each configured `read_metric`'s **last decoded value** stamped with
the **device's source timestamp** — never aggregated. The metrics poll
**skips** valve-class devices so the stream is the sole, authoritative writer.
So with the valve command configured as `command_class = "valve"`, your
configured valve `read_metric`s (`valveStatusCode`, `state`, `valvePosition`,
`moving`, `fault`, `lowBattery`) show their true last values — `valveStatusCode`
reads a clean `195` (closed), not the aggregated `391`.

Configure them as normal `read_metric` entries (the `chirpstack_metric_name`
must match the codec's decoded-object field name):

```toml
[[application.device.read_metric]]
metric_name            = "Status_v01"
chirpstack_metric_name = "valveStatusCode"   # codec field; stored via the event stream
metric_type            = "Int"
```

> **E-1b implemented:** non-valve devices migrate off the aggregated poll by
> setting `chirpstack.stream_all_devices = true` (validated on v2.2.0-rc2
> pre-prod), and on startup/reconnect opcgw backfills the last value from the
> device's recent event history (timestamp-guarded, never `GetMetrics`).
> E-1 is a **v2.2.0 release blocker** (Epic E / issues #129, #130) — final
> fleet sign-off on a release candidate is the remaining `done` gate.

The integer fields the codec emits (`valveStatusCode`, `valvePosition`,
`moving`, `fault`, `lowBattery`) are the values E-1 will read from each event —
they are correct *at the source*; the corruption is introduced only by the
metrics-poll aggregation in front of them.
