package com.bridgething.companion.shell

import io.ktor.server.application.Application
import io.ktor.server.cio.CIO
import io.ktor.server.engine.EmbeddedServer
import io.ktor.server.engine.embeddedServer
import java.net.InetAddress
import java.net.ServerSocket
import kotlinx.coroutines.runBlocking

internal const val LOOPBACK = "127.0.0.1"

internal fun loopbackServer(module: Application.() -> Unit): Pair<EmbeddedServer<*, *>, Int> {
    val srv = embeddedServer(CIO, port = 0, host = LOOPBACK, module = module).start(wait = false)
    val port = runBlocking { srv.engine.resolvedConnectors().first().port }
    return srv to port
}

internal fun closedLoopbackPort(): Int = ServerSocket(0, 0, InetAddress.getByName(LOOPBACK)).use { it.localPort }
