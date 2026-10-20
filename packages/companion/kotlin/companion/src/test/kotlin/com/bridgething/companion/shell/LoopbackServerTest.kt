package com.bridgething.companion.shell

import io.ktor.server.response.respondText
import io.ktor.server.routing.get
import io.ktor.server.routing.routing
import java.net.BindException
import java.net.InetSocketAddress
import java.net.ServerSocket
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Test

class LoopbackServerTest {
    @Test
    fun anotherListenerCannotShadowTheDialedAddress() {
        val (srv, port) = loopbackServer { routing { get("/") { call.respondText("ok") } } }
        try {
            ServerSocket().use { intruder ->
                intruder.reuseAddress = true
                assertThrows(BindException::class.java) { intruder.bind(InetSocketAddress(LOOPBACK, port)) }
            }
        } finally {
            srv.stop(0, 0)
        }
    }
}
