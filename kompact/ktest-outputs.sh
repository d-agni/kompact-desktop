#!/bin/sh
# Phase 5: ./ktest-desktop.sh with KOMPACT_OUTPUTS set. The mode must be given (no default, so a
# run can't silently land in the wrong one).
#   ./ktest-outputs.sh observe|reconnect|all [wallpaper image]
# Afterwards: sh tools/check-outputs.sh > check-outputs.log  (and sh tools/check-vt.sh)
cd "$(dirname "$0")"
case "${1:-}" in
    observe) what="one screen (the primary), monitors logged (5a)" ;;
    reconnect) what="one screen, set up again after unplug/replug (5b)" ;;
    all) what="EVERY connected screen, side by side (5c/5d)" ;;
    *)
        echo "usage: $0 observe|reconnect|all [wallpaper image]"
        echo "  observe    one screen (the primary), monitors logged (5a)"
        echo "  reconnect  one screen, set up again after unplug/replug (5b)"
        echo "  all        every connected screen, side by side (5c/5d)"
        exit 1
        ;;
esac
mode=$1
shift
echo "kompact with KOMPACT_OUTPUTS=$mode: $what (log: ktest.log)"
KOMPACT_OUTPUTS=$mode ./ktest-desktop.sh "$@"
