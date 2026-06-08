<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- (c) [2026] Guy Corbaz -->

# Story E-0 — Real-world valve test runbook (AC#10)

Goal: prove that an **OPC UA write → opcgw → ChirpStack downlink → physical Tonhe
E20 valve actuates** end-to-end. Automated tests + clippy passing is **not**
sufficient for this story (main-deadlock incident doctrine); this manual test is
the only gate left before E-0 → `done`.

Setup assumed (confirmed 2026-06-07): **local dev build** against your **real
ChirpStack**, valve **already joined** in ChirpStack (codec installed), driving
the write with the bundled **Python script**.

---

## 1. Configure the valve command in opcgw

Add a command block to the valve device in the config you run locally
(`config/config.toml`, or whatever `-c` file points at your real ChirpStack).
Use the valve's real DevEUI as `device_id` and its real `application_id`:

```toml
[[application]]
application_id = "<YOUR_REAL_APPLICATION_ID>"
application_name = "Valves"

    [[application.device]]
    device_name = "Valve1"
    device_id   = "<YOUR_VALVE_DEVEUI>"   # e.g. a84041b8a1867e20

        # E-0 valve command — write 1 (open) / 0 (close) to this OPC UA node.
        [[application.device.command]]
        command_id        = 1
        command_name      = "Valve"
        command_confirmed = true          # Tonhe sends a confirm packet
        command_port      = 10            # fPort 10 (Tonhe open/close port)
        command_class     = "valve"       # <-- the E-0 semantic-object path
```

`command_class = "valve"` is the load-bearing line: it makes opcgw enqueue a
`{"command":"open"}` / `{"command":"close"}` **object** (codec → bytes) instead
of the raw OPC UA value. Without it, `0` would send raw byte `0x00`, which is
**not** a valid valve command — close would never work.

> The command node's OPC UA NodeId is `ns=<urn:UpcUaG>;s=<device_id>/<command_id>`
> — e.g. `s=a84041b8a1867e20/1`. The script resolves the namespace index for you.

## 2. Run opcgw against the real ChirpStack

Provide the secrets via env (they are placeholder-rejected in the TOML):

```bash
export OPCGW_CHIRPSTACK__API_TOKEN="<your-chirpstack-api-token>"
export OPCGW_OPCUA__USER_PASSWORD="<choose-an-opcua-password>"
# ...and point [chirpstack].server_address at your real ChirpStack (e.g.
#    http://192.168.1.12:8080) and set the correct tenant_id in the TOML.

# Easiest auth for a local test: the security-None endpoint (still needs the
# username/password above). For dev TLS, set [opcua].create_sample_keypair=true.

./target/release/opcgw -c config/config.toml
```

Confirm in the log that the OPC UA server bound its port (`4840`) and the
ChirpStack poller connected (a poll cycle fires). Leave it running.

## 3. Drive the valve from OPC UA

```bash
pip install asyncua    # one-time

export OPCGW_OPCUA__USER_NAME="opcua-user"          # matches [opcua].user_name
export OPCGW_OPCUA__USER_PASSWORD="<same-as-above>"

# OPEN, pause to press SET, then CLOSE — the whole AC#10 cycle:
python3 "docs/LoRa/TONHE Valve/valve_opcua_test.py" \
    --device <YOUR_VALVE_DEVEUI> --command-id 1 cycle

# or one direction at a time:
python3 "docs/LoRa/TONHE Valve/valve_opcua_test.py" --device <DEVEUI> open
python3 "docs/LoRa/TONHE Valve/valve_opcua_test.py" --device <DEVEUI> close
```

(`--endpoint` defaults to `opc.tcp://127.0.0.1:4840/`; override if opcgw runs
elsewhere.)

## 4. Force delivery + observe (Class A)

The valve is **Class A** — a queued downlink lands only on its next uplink.
**Press the SET button on the valve** to force an immediate report+pull (the
script's `cycle` mode pauses so you can press it).

Verify the path:

1. **opcgw log** — `deliver` / Enqueue debug lines, command status `Pending → Sent`.
2. **ChirpStack → Device → Queue** — the downlink appears, then clears once sent.
3. **ChirpStack → Device → Events** — decoded uplink `state` flips to
   `opening`/`open` (after OPEN) or `closing`/`closed` (after CLOSE).
4. **The physical valve** moves. ✅

## 5. Record the result → flip E-0 to done

Once OPEN **and** CLOSE both actuate the real valve:

- Tick **Task 8** in `_bmad-output/implementation-artifacts/E-0-downlink-command-path.md`
  and add the outcome to its Completion Notes (date, what you observed).
- Flip `E-0-downlink-command-path: review → done` in `sprint-status.yaml`.
- Commit the review-fix changes + this result, then `git push`
  (2 commits are currently unpushed: `6a29c6e` impl + `e59c6e7` review).

If it does **not** actuate, capture the opcgw log + ChirpStack Events and we
debug before flipping — a delayed actuation (waiting for wake-up) is **not** a
failure; only a non-actuation after a SET press is.

---

### Quick troubleshooting

| Symptom | Likely cause |
|---|---|
| Script: `BadUserAccessDenied` / connect refused | username/password mismatch with opcgw's `[opcua]` user / `OPCGW_OPCUA__USER_PASSWORD` |
| Script: `BadNodeIdUnknown` | wrong `--device`/`--command-id`, or the command block isn't in the running config |
| `Sent` in opcgw but no valve movement | Class A — press SET; or the device profile codec isn't installed / fPort ≠ 10 |
| Status goes `Failed` | check opcgw ERROR log — bad fPort, ChirpStack auth, or device not in that application |
| CLOSE does nothing but OPEN works | `command_class = "valve"` missing → `0` sent as raw `0x00` (invalid). Add the flag. |
