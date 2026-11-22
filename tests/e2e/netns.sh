#!/bin/bash
# Real-kernel end-to-end test for tvpn using network namespaces.
#
#   bash tests/e2e/netns.sh target/debug/tvpn
#
# Topology (all throwaway namespaces, nothing on the host is touched):
#
#   client (B)  veth1 192.168.77.2 ── veth0 192.168.77.1  server (A)  wan0 198.51.100.1 ── wan1 198.51.100.2  internet (C)
#
# Checks: handshake, full-tunnel routes, endpoint bypass route, DNS rewrite and
# restore (B gets a private copy of /etc), traffic to the server, traffic to
# the "internet" through server forwarding + NAT, status/--json, live peer
# reload, clean teardown, and crash recovery after kill -9.
#
# Uses unprivileged user namespaces when available; otherwise run with sudo.
set -u

BIN="$(readlink -f "${1:?usage: netns.sh <path-to-tvpn>}")"
WORK="$(mktemp -d /tmp/tvpn-e2e.XXXXXX)"
export TVPN_CONFIG_DIR="$WORK/cfg" TVPN_NO_SUDO=1 NO_COLOR=1

"$BIN" server init --endpoint 192.168.77.1 --port 51820 >/dev/null
"$BIN" server add-peer laptop -o "$WORK/laptop.conf" >/dev/null

cat > "$WORK/inner.sh" <<'EOF'
set -u
BIN="$1"; WORK="$2"
FAILS=0
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; FAILS=$((FAILS + 1)); }
check() { local desc="$1"; shift; if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi; }

ip link set lo up
unshare -nm --propagation private sleep 600 & B_PID=$!
unshare -n sleep 600 & C_PID=$!
sleep 0.3
B() { nsenter -t "$B_PID" -n -m "$@"; }
C() { nsenter -t "$C_PID" -n "$@"; }
SRV() { TVPN_RUNTIME_DIR="$WORK/run-s" "$BIN" "$@"; }
CLI() { B env TVPN_RUNTIME_DIR="$WORK/run-c" "$BIN" "$@"; }

ip link add veth0 type veth peer name veth1; ip link set veth1 netns "$B_PID"
ip link add wan0 type veth peer name wan1;   ip link set wan1 netns "$C_PID"
ip addr add 192.168.77.1/24 dev veth0; ip link set veth0 up
ip addr add 198.51.100.1/24 dev wan0; ip link set wan0 up
ip route add default via 198.51.100.2
B sh -c 'ip link set lo up; ip addr add 192.168.77.2/24 dev veth1; ip link set veth1 up; ip route add default via 192.168.77.1'
C sh -c 'ip link set lo up; ip addr add 198.51.100.2/24 dev wan1; ip link set wan1 up'
mkdir -p "$WORK/etc"; cp -a /etc/. "$WORK/etc/" 2>/dev/null
printf 'nameserver 192.0.2.53\n' > "$WORK/etc/resolv.conf.orig"
rm -f "$WORK/etc/resolv.conf"; cp "$WORK/etc/resolv.conf.orig" "$WORK/etc/resolv.conf"
B mount --bind "$WORK/etc" /etc

FWD_BEFORE="$(cat /proc/sys/net/ipv4/ip_forward)"
SRV server up > "$WORK/server.log" 2>&1 & SERVER=$!
sleep 1
CLI up "$WORK/laptop.conf" > "$WORK/client.log" 2>&1 & CLIENT=$!
sleep 3

check "client reports Connected"            grep -q "Connected" "$WORK/client.log"
check "route 0.0.0.0/1 via tvpn0"           sh -c "nsenter -t $B_PID -n ip -4 route | grep -q '^0.0.0.0/1 dev tvpn0'"
check "route 128.0.0.0/1 via tvpn0"         sh -c "nsenter -t $B_PID -n ip -4 route | grep -q '^128.0.0.0/1 dev tvpn0'"
check "endpoint bypass route"               sh -c "nsenter -t $B_PID -n ip -4 route | grep -q '^192.168.77.1 dev veth1'"
check "resolv.conf rewritten"               sh -c "nsenter -t $B_PID -n -m grep -q 'nameserver 1.1.1.1' /etc/resolv.conf"
check "ping server through tunnel"          B ping -c 2 -W 2 10.8.0.1
check "ping internet host via server NAT"   B ping -c 2 -W 2 198.51.100.2
check "server enabled ip_forward"           sh -c "[ \"\$(cat /proc/sys/net/ipv4/ip_forward)\" = 1 ]"
check "server added MASQUERADE rule"        sh -c "iptables -t nat -S POSTROUTING | grep -q MASQUERADE"
check "status --json says connected"        sh -c "nsenter -t $B_PID -n -m env TVPN_RUNTIME_DIR=$WORK/run-c $BIN status --json | grep -q '\"connected\": true'"
check "server status lists online peer"     sh -c "TVPN_RUNTIME_DIR=$WORK/run-s $BIN status | grep -q '1 of 1 peers online'"
check "add-peer reloads running server"     sh -c "TVPN_RUNTIME_DIR=$WORK/run-s TVPN_CONFIG_DIR=$WORK/cfg $BIN server add-peer phone | grep -q '1 added'"
check "tvpn down"                           CLI down
sleep 0.5
check "client process exited"               sh -c "! kill -0 $CLIENT"
check "tunnel routes removed"               sh -c "! nsenter -t $B_PID -n ip -4 route | grep -q tvpn0"
check "bypass route removed"                sh -c "! nsenter -t $B_PID -n ip -4 route | grep -q '^192.168.77.1 '"
check "resolv.conf restored"                B cmp -s /etc/resolv.conf /etc/resolv.conf.orig
check "tvpn0 interface removed"             sh -c "! nsenter -t $B_PID -n ip link show tvpn0"

# Crash recovery: kill -9 a connected client, then `tvpn down` cleans up.
CLI up "$WORK/laptop.conf" > "$WORK/client2.log" 2>&1 & CLIENT=$!
sleep 2
kill -9 "$CLIENT"; sleep 0.3
check "after kill -9, resolv.conf still changed" sh -c "nsenter -t $B_PID -n -m grep -q tvpn /etc/resolv.conf"
check "tvpn down recovers crashed session"  CLI down
check "resolv.conf restored after crash"    B cmp -s /etc/resolv.conf /etc/resolv.conf.orig
check "bypass route removed after crash"    sh -c "! nsenter -t $B_PID -n ip -4 route | grep -q '^192.168.77.1 '"

check "server down"                         SRV server down
wait "$SERVER" 2>/dev/null
check "ip_forward restored"                 sh -c "[ \"\$(cat /proc/sys/net/ipv4/ip_forward)\" = $FWD_BEFORE ]"
check "firewall rules removed"              sh -c "! iptables -S | grep -q tvpns0 && ! iptables -t nat -S | grep -q MASQUERADE"

kill "$B_PID" "$C_PID" 2>/dev/null
if [ "$FAILS" -gt 0 ]; then
    echo "---- server.log"; cat "$WORK/server.log"
    echo "---- client.log"; cat "$WORK/client.log"
    echo "$FAILS check(s) failed"; exit 1
fi
echo "all checks passed"
EOF

if [ "$(id -u)" = 0 ]; then
    unshare -n bash "$WORK/inner.sh" "$BIN" "$WORK"
else
    unshare -Urn --kill-child bash "$WORK/inner.sh" "$BIN" "$WORK"
fi
status=$?
rm -rf "$WORK"
exit $status
