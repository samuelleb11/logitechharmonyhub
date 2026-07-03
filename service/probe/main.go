// probe: a minimal binary to prove Go runs on the Hub's Linux 2.6.31 kernel.
// It exercises the parts most likely to hit Go's kernel-floor (>=2.6.32) issue:
// the goroutine scheduler, channels, and especially the network poller
// (epoll_create1 / eventfd2) via a real TCP listen+dial over loopback.
// Stdlib only; kept import-light (net, os) so the binary stays small.
package main

import (
	"net"
	"os"
)

func say(s string) { os.Stdout.WriteString(s) }

func main() {
	say("GO_START\n")

	// file I/O syscalls
	if b, err := os.ReadFile("/proc/version"); err == nil {
		say("KVER: " + string(b))
	} else {
		say("KVER_ERR: " + err.Error() + "\n")
	}

	// network poller: listen on loopback (lo only is fine)
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		say("LISTEN_FAIL: " + err.Error() + "\n")
		os.Exit(1)
	}
	say("LISTEN_OK " + ln.Addr().String() + "\n")

	done := make(chan string, 1)
	go func() {
		c, e := ln.Accept()
		if e != nil {
			done <- "ACCEPT_ERR:" + e.Error()
			return
		}
		buf := make([]byte, 16)
		n, _ := c.Read(buf)
		c.Write([]byte("pong"))
		c.Close()
		done <- "GOT:" + string(buf[:n])
	}()

	c, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		say("DIAL_FAIL: " + err.Error() + "\n")
		os.Exit(1)
	}
	c.Write([]byte("ping"))
	rb := make([]byte, 16)
	n, _ := c.Read(rb)
	say("DIAL_GOT: " + string(rb[:n]) + "\n")
	say("SRV_" + (<-done) + "\n")

	say("GO_RUNS_2631_OK\n")
	os.Exit(0)
}
