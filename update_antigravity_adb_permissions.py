#!/usr/bin/env python3
"""
update_antigravity_adb_permissions.py

Updates Antigravity CLI and Gemini configuration files to always allow
ADB commands without requiring interactive approval for every command.
"""

import json
import os
import shutil
import sys
from pathlib import Path

# Target configuration files used by Antigravity CLI / AGY and Gemini
CONFIG_TARGETS = [
    Path.home() / ".gemini" / "antigravity-cli" / "settings.json",
    Path.home() / ".gemini" / "settings.json",
]

# Permission patterns for ADB commands
# Covers standard 'adb ...', flags, subcommands, and full paths to adb
ADB_ALLOW_RULES = [
    "command(adb*)",
    "command(regex:^adb(\\s+.*)?$)",
    "command(regex:^.*[/\\\\]adb(\\s+.*)?$)",
]


def update_config_file(file_path: Path) -> bool:
    """Updates a settings.json file to add ADB rules under permissions.allow."""
    print(f"\nChecking: {file_path}")

    # Ensure parent directory exists
    file_path.parent.mkdir(parents=True, exist_ok=True)

    data = {}
    if file_path.exists():
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read().strip()
                if content:
                    data = json.loads(content)
        except json.JSONDecodeError as e:
            print(f"  [!] Error parsing JSON in {file_path}: {e}")
            return False
        except Exception as e:
            print(f"  [!] Error reading {file_path}: {e}")
            return False

        # Create a timestamped/safe backup
        backup_file = file_path.with_suffix(".json.bak")
        try:
            shutil.copy2(file_path, backup_file)
            print(f"  [*] Created backup at: {backup_file}")
        except Exception as e:
            print(f"  [!] Failed to create backup: {e}")

    # Ensure "permissions" object exists
    if "permissions" not in data or not isinstance(data["permissions"], dict):
        data["permissions"] = {}

    # Ensure "allow" list exists
    if "allow" not in data["permissions"] or not isinstance(data["permissions"]["allow"], list):
        data["permissions"]["allow"] = []

    allow_list = data["permissions"]["allow"]
    added_rules = []

    for rule in ADB_ALLOW_RULES:
        if rule not in allow_list:
            allow_list.append(rule)
            added_rules.append(rule)

    if not added_rules:
        print(f"  [-] ADB permission rules already present in {file_path}")
        return True

    for r in added_rules:
        print(f"  [+] Added: {r}")

    # Write back updated settings with clean formatting
    try:
        with open(file_path, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=4)
            f.write("\n")
        print(f"  [✓] Updated {file_path} successfully.")
        return True
    except Exception as e:
        print(f"  [!] Failed to write {file_path}: {e}")
        return False


def main():
    print("=" * 60)
    print("  Antigravity CLI: Always Allow ADB Commands Updater")
    print("=" * 60)

    success_count = 0
    for target in CONFIG_TARGETS:
        if update_config_file(target):
            success_count += 1

    print("\n" + "=" * 60)
    print(f"Finished processing {len(CONFIG_TARGETS)} configuration file(s).")
    print("Rule changes will take effect on your next Antigravity session.")
    print("=" * 60)


if __name__ == "__main__":
    main()
