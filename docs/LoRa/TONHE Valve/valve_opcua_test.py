#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# (c) [2026] Guy Corbaz
"""
E-0 valve test helper — write the canonical OPEN/CLOSE value to an opcgw
command node over OPC UA, so a real LoRaWAN downlink is enqueued to ChirpStack.

This is the operator side of Story E-0 AC#10 (real-world valve gate). opcgw maps
the value you write here:  1 -> {"command":"open"}  /  0 -> {"command":"close"}
and the ChirpStack device-profile codec turns that into fPort-10 bytes
0x01 / 0x02. (The valve is Class A — press SET on the valve to force immediate
delivery; see the runbook.)

The opcgw command node is an Int32, writable, with string NodeId
    ns=<urn:UpcUaG>;s=<device_id>/<command_id>
Every opcgw endpoint (even security None) requires the configured
username/password, so this script always supplies them.

Usage
-----
    pip install asyncua            # one-time

    # OPEN valve (writes Int32 1)
    python3 valve_opcua_test.py --device a84041b8a1867e20 --command-id 1 open

    # CLOSE valve (writes Int32 0)
    python3 valve_opcua_test.py --device a84041b8a1867e20 --command-id 1 close

    # Full OPEN -> wait -> CLOSE cycle with a pause to press SET between writes
    python3 valve_opcua_test.py --device a84041b8a1867e20 --command-id 1 cycle

Credentials come from --user / --password or the env vars
OPCGW_OPCUA__USER_NAME / OPCGW_OPCUA__USER_PASSWORD (matching how you run opcgw).
"""

import argparse
import asyncio
import os
import sys

try:
    from asyncua import Client, ua
except ImportError:
    sys.exit("asyncua not installed. Run:  pip install asyncua")

NS_URI = "urn:UpcUaG"  # opcgw namespace (src/utils.rs OPCUA_NAMESPACE_URI)

ACTION_VALUE = {"open": 1, "close": 0}  # canonical OPC UA values (NOT wire bytes)


async def write_command(client: Client, device: str, command_id: int, value: int) -> None:
    ns = await client.get_namespace_index(NS_URI)
    node_id = f"ns={ns};s={device}/{command_id}"
    node = client.get_node(node_id)
    name = await node.read_browse_name()
    print(f"  node   : {node_id}  ({name.Name})")
    dv = ua.DataValue(ua.Variant(int(value), ua.VariantType.Int32))
    await node.write_value(dv)
    semantic = '{"command":"open"}' if value == 1 else '{"command":"close"}'
    print(f"  wrote  : Int32 {value}  -> opcgw will enqueue {semantic}")
    readback = await node.read_value()
    print(f"  readback: {readback}")


async def main() -> int:
    ap = argparse.ArgumentParser(description="opcgw E-0 valve OPC UA test writer")
    ap.add_argument("action", choices=["open", "close", "cycle"],
                    help="open (write 1), close (write 0), or cycle (open, pause, close)")
    ap.add_argument("--endpoint", default=os.environ.get("OPCGW_ENDPOINT", "opc.tcp://127.0.0.1:4840/"),
                    help="OPC UA endpoint URL (default opc.tcp://127.0.0.1:4840/)")
    ap.add_argument("--device", required=True, help="device_id (DevEUI) as configured in opcgw")
    ap.add_argument("--command-id", type=int, default=1, help="command_id from the [[application.device.command]] block (default 1)")
    ap.add_argument("--user", default=os.environ.get("OPCGW_OPCUA__USER_NAME", "opcua-user"),
                    help="OPC UA username (default env OPCGW_OPCUA__USER_NAME or 'opcua-user')")
    ap.add_argument("--password", default=os.environ.get("OPCGW_OPCUA__USER_PASSWORD"),
                    help="OPC UA password (default env OPCGW_OPCUA__USER_PASSWORD)")
    ap.add_argument("--cycle-pause", type=float, default=30.0,
                    help="seconds to wait between open and close in 'cycle' mode (press SET on the valve during this window)")
    args = ap.parse_args()

    if not args.password:
        return _fail("No password. Pass --password or set OPCGW_OPCUA__USER_PASSWORD "
                     "(must match the value opcgw is running with).")

    print(f"Connecting to {args.endpoint} as '{args.user}' ...")
    client = Client(url=args.endpoint)
    client.set_user(args.user)
    client.set_password(args.password)
    # Security None endpoint — opcgw exposes it but still enforces user/pass.
    try:
        async with client:
            print("Connected.\n")
            if args.action in ("open", "close"):
                print(f"--- {args.action.upper()} ---")
                await write_command(client, args.device, args.command_id, ACTION_VALUE[args.action])
            else:  # cycle
                print("--- OPEN ---")
                await write_command(client, args.device, args.command_id, 1)
                print(f"\n>>> Press the SET button on the valve now to force the "
                      f"Class-A downlink. Waiting {args.cycle_pause:.0f}s ...\n")
                await asyncio.sleep(args.cycle_pause)
                print("--- CLOSE ---")
                await write_command(client, args.device, args.command_id, 0)
                print("\n>>> Press SET again to deliver the CLOSE downlink.")
    except Exception as e:  # noqa: BLE001 - operator-facing tool, surface anything
        return _fail(f"{type(e).__name__}: {e}")

    print("\nDone. Watch ChirpStack -> Device -> Events for the decoded uplink "
          "(state flips to opening/open or closing/closed), and watch the opcgw "
          "log for the 'deliver' / Enqueue debug lines.")
    return 0


def _fail(msg: str) -> int:
    print(f"ERROR: {msg}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
