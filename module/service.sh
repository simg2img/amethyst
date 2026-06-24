#!/system/bin/sh
MODDIR="${0%/*}"
nohup "$MODDIR/lib/daemon" >/dev/null 2>&1 &
