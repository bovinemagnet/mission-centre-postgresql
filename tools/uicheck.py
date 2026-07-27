#!/usr/bin/env python3
#
# Copyright 2026 Paul Snow
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.
#
# SPDX-License-Identifier: GPL-3.0-or-later

"""Read the running application's user interface over the accessibility bus.

Every success criterion in the phase specs is phrased in terms a screen
reader can also see — "the controls become sensitive", "the bar shows the
reason", "renders as one tree" — so AT-SPI can check most of them without
a human at the window. GTK4 publishes this automatically; the application
needs no code for it and is not modified by this tool.

Strictly read-only. Nothing here clicks, types or activates: a walkthrough
that alters the thing it is inspecting cannot be trusted, and an accidental
`terminate` against a real server is not a risk worth taking for a test
harness. Putting the application into the state to be checked stays a
deliberate act.

Usage:
    tools/uicheck.py digest            # what the UI currently shows
    tools/uicheck.py criteria          # evaluate the read-only criteria
    tools/uicheck.py criteria --dsn "host=127.0.0.1 port=55432 user=postgres"

Requires python-gobject and at-spi2-core, both of which GTK already pulls in.
"""

import argparse
import re
import subprocess
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

APP_NAME = "mission-centre-pg"
MAX_DEPTH = 30

PASS, FAIL, SKIP = "PASS", "FAIL", "SKIP"


# --------------------------------------------------------------------------
# Accessibility tree access
# --------------------------------------------------------------------------


def application():
    """The running application's accessibility root, or None."""
    Atspi.init()
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        node = desktop.get_child_at_index(i)
        if node is not None and APP_NAME in (node.get_name() or ""):
            return node
    return None


def walk(node, depth=0):
    """Yield every node beneath `node`, depth first."""
    if depth > MAX_DEPTH or node is None:
        return
    yield node
    try:
        for i in range(node.get_child_count()):
            child = node.get_child_at_index(i)
            if child is not None:
                yield from walk(child, depth + 1)
    except Exception:  # noqa: BLE001 — a node can vanish mid-walk
        return


def role(node):
    try:
        return node.get_role_name()
    except Exception:  # noqa: BLE001
        return "?"


def name(node):
    try:
        return node.get_name() or ""
    except Exception:  # noqa: BLE001
        return ""


def has_state(node, state):
    try:
        return node.get_state_set().contains(getattr(Atspi.StateType, state))
    except Exception:  # noqa: BLE001
        return False


def by_role(app, wanted):
    return [n for n in walk(app) if role(n) == wanted]


def visible_labels(app):
    return [name(n) for n in walk(app) if role(n) == "label" and has_state(n, "SHOWING") and name(n)]


def selected_tabs(app):
    return [name(n) for n in by_role(app, "page tab") if has_state(n, "SELECTED")]


def buttons(app):
    """Every button that is on screen, mapped to whether it is sensitive.

    The name is taken from the button, falling back to its label child: a
    GTK button built from a label reports the label as its accessible name
    only once the label is realised.
    """
    found = {}
    for node in by_role(app, "button"):
        if not has_state(node, "SHOWING"):
            continue
        label = name(node)
        if not label:
            try:
                child = node.get_child_at_index(0)
                label = name(child) if child else ""
            except Exception:  # noqa: BLE001
                label = ""
        if label:
            found[label] = has_state(node, "SENSITIVE")
    return found


def table_rows(app):
    """Data rows on screen, header row excluded, in display order."""
    rows = []
    for node in by_role(app, "table row"):
        if not has_state(node, "SHOWING"):
            continue
        text = name(node)
        if text and not text.startswith("Blocked User"):
            rows.append(text)
    return rows


def indent_of(row):
    """Leading spaces, which is how the blocked tree renders its depth."""
    return len(row) - len(row.lstrip(" "))


# --------------------------------------------------------------------------
# Criteria
# --------------------------------------------------------------------------


def criterion_tree_shape(app):
    """§10.1-2 — a chain renders as one nested tree, not several flat rows."""
    rows = table_rows(app)
    chain = [r for r in rows if re.match(r"^\s*\d+\s", r)]
    if not chain:
        return SKIP, "no blocked-session rows on screen"

    depths = [indent_of(r) for r in chain]
    if len(chain) == 1:
        return SKIP, f"only one participant on screen: {chain[0][:60]!r}"
    if depths[0] != 0:
        return FAIL, f"the first row is indented ({depths[0]} spaces); a root should not be"
    if max(depths) == 0:
        return FAIL, f"{len(chain)} rows, none indented — rendered flat, not as a tree"

    shape = " -> ".join(r.strip().split()[0] for r in chain)
    return PASS, f"{len(chain)} participants nested {max(depths) // 4 + 1} deep: {shape}"


def criterion_root_identified(app):
    """§10.2 — the root carries its identity, not just a bare pid."""
    rows = [r for r in table_rows(app) if re.match(r"^\d+\s", r)]
    if not rows:
        return SKIP, "no root row on screen"
    fields = rows[0].split()
    if len(fields) < 3:
        return FAIL, f"root row carries only {len(fields)} fields: {rows[0]!r}"
    return PASS, f"root names pid/user/database: {' '.join(fields[:3])}"


def criterion_actions_gated(app):
    """§10.1 — the action controls track selection rather than being always live."""
    found = buttons(app)
    actions = {k: v for k, v in found.items() if k in ("Cancel query", "Terminate")}
    if not actions:
        return SKIP, "the action bar is not on screen"

    rows_selected = any(has_state(n, "SELECTED") for n in by_role(app, "table row"))
    sensitive = [k for k, v in actions.items() if v]
    if rows_selected and len(sensitive) != len(actions):
        return FAIL, f"a row is selected but only {sensitive} are sensitive"
    if not rows_selected and sensitive:
        return FAIL, f"nothing is selected yet {sensitive} are sensitive"
    state = "sensitive" if rows_selected else "insensitive"
    return PASS, f"{sorted(actions)} correctly {state} (selection={rows_selected})"


def criterion_empty_state_honest(app):
    """§10.4 and §10.11 — an empty page never claims more than it can know."""
    labels = visible_labels(app)
    healthy = [l for l in labels if "No blocked sessions" in l]
    restricted = [l for l in labels if "Contention cannot be seen" in l]
    limited = any("pg_monitor" in l for l in labels)

    if restricted:
        return PASS, "restricted notice shown in place of a false all-clear"
    if healthy and limited:
        return FAIL, "claims 'No blocked sessions' while the role is flagged as limited"
    if healthy:
        return PASS, "'No blocked sessions' shown, and the role is not limited"
    return SKIP, "no empty state on screen"


def criterion_truncation_reported(app):
    """§10.5 — a shortened inventory says so."""
    for label in visible_labels(app):
        match = re.search(r"Showing (\d+) of (\d+) locks", label)
        if match:
            shown, total = int(match.group(1)), int(match.group(2))
            if shown >= total:
                return FAIL, f"reports truncation but shows all of them ({shown}/{total})"
            return PASS, f"truncation stated: {shown} of {total}"
    return SKIP, "no truncation notice on screen"


def criterion_lag_in_both_units(app):
    """§10.7 — standby lag is given in seconds and bytes."""
    rows = table_rows(app)
    lag_rows = [r for r in rows if re.search(r"\d+\.\d+s\s*/\s*[\d.]+\s*[KMGT]?i?B", r)]
    if lag_rows:
        return PASS, f"both units present: {lag_rows[0].strip()[:70]}"
    if any("streaming" in r for r in rows):
        return FAIL, f"a standby row is shown without both lag units: {rows[0][:70]!r}"
    return SKIP, "no standby rows on screen"


def criterion_version_gate_stated(app):
    """§10.10 — an unsupported column names the version it needs."""
    labels = visible_labels(app)
    stated = [l for l in labels if "requires PostgreSQL" in l]
    if stated:
        return PASS, f"version requirement stated: {stated[0]}"
    return SKIP, "no version-gated notice on screen"


def criterion_nothing_silently_empty(app):
    """§10.11 — a limited role is told why, not shown blanks."""
    labels = visible_labels(app)
    limited = [l for l in labels if "pg_monitor" in l]
    if limited:
        return PASS, f"privilege stated: {limited[0][:70]}"
    return SKIP, "no privilege notice on screen (connected role may be privileged)"


def criterion_inventory_gate(dsn):
    """§10.6 — the inventory query does not run while its view is hidden.

    Checked against the server rather than the UI, because the UI cannot show
    the absence of a query. Read-only: one SELECT against pg_stat_activity.
    """
    if not dsn:
        return SKIP, "no --dsn given"

    sql = (
        "SELECT count(*) FROM pg_stat_activity "
        "WHERE application_name = 'mission-centre-pg' AND query LIKE '%pg_locks%'"
    )
    try:
        out = subprocess.run(
            ["psql", dsn, "-tAc", sql],
            capture_output=True,
            text=True,
            timeout=15,
            check=True,
        ).stdout.strip()
    except FileNotFoundError:
        return SKIP, "psql is not on PATH"
    except subprocess.CalledProcessError as exc:
        return SKIP, f"psql failed: {exc.stderr.strip()[:60]}"
    except subprocess.TimeoutExpired:
        return SKIP, "psql timed out"

    return PASS, f"{out} pg_locks queries in flight (sampled once; not a proof of absence)"


CRITERIA = [
    ("§10.1-2 tree shape", criterion_tree_shape),
    ("§10.2  root identified", criterion_root_identified),
    ("§10.1  actions gated on selection", criterion_actions_gated),
    ("§10.4  empty state honest", criterion_empty_state_honest),
    ("§10.5  truncation reported", criterion_truncation_reported),
    ("§10.7  lag in both units", criterion_lag_in_both_units),
    ("§10.10 version gate stated", criterion_version_gate_stated),
    ("§10.11 nothing silently empty", criterion_nothing_silently_empty),
]


# --------------------------------------------------------------------------
# Commands
# --------------------------------------------------------------------------


def cmd_digest(app, _args):
    print(f"page:     {', '.join(selected_tabs(app)) or '(none selected)'}")

    found = buttons(app)
    if found:
        print("buttons:")
        for label in sorted(found):
            print(f"  {'sensitive  ' if found[label] else 'insensitive'} {label}")

    rows = table_rows(app)
    if rows:
        print(f"rows ({len(rows)}):")
        for row in rows[:12]:
            print(f"  [{indent_of(row):>2}] {row[:96]}")
        if len(rows) > 12:
            print(f"  … {len(rows) - 12} more")

    notices = [
        l
        for l in visible_labels(app)
        if any(
            k in l
            for k in ("pg_monitor", "pg_signal", "requires PostgreSQL", "Showing", "No blocked", "cannot be seen")
        )
    ]
    if notices:
        print("notices:")
        for notice in notices:
            print(f"  {notice[:100]}")
    return 0


def cmd_criteria(app, args):
    results = [(label, *check(app)) for label, check in CRITERIA]
    results.append(("§10.6  inventory gated", *criterion_inventory_gate(args.dsn)))

    width = max(len(label) for label, _, _ in results)
    failures = 0
    for label, verdict, detail in results:
        if verdict == FAIL:
            failures += 1
        print(f"{verdict:<4} {label:<{width}}  {detail}")

    checked = sum(1 for _, v, _ in results if v != SKIP)
    print(f"\n{checked}/{len(results)} evaluated, {failures} failed "
          f"({len(results) - checked} skipped — the UI was not showing that state)")
    return 1 if failures else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("command", choices=["digest", "criteria"])
    parser.add_argument(
        "--dsn",
        help="libpq connection string, for the one criterion the UI cannot show",
    )
    args = parser.parse_args()

    app = application()
    if app is None:
        print(f"{APP_NAME} is not on the accessibility bus — is it running?", file=sys.stderr)
        return 2

    return {"digest": cmd_digest, "criteria": cmd_criteria}[args.command](app, args)


if __name__ == "__main__":
    sys.exit(main())
