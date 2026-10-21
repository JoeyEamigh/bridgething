package com.bridgething.companion.shell

import io.ktor.server.application.install
import io.ktor.server.routing.routing
import io.ktor.server.websocket.DefaultWebSocketServerSession
import io.ktor.server.websocket.WebSockets
import io.ktor.server.websocket.webSocket
import io.ktor.websocket.CloseReason
import io.ktor.websocket.Frame
import io.ktor.websocket.close
import io.ktor.websocket.readText
import java.net.InetAddress
import java.net.ServerSocket
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.delay
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import uniffi.bridgething_companion.NoHandle
import uniffi.bridgething_companion.WsConnect
import uniffi.bridgething_companion.WsFrame
import uniffi.bridgething_companion.WsInbox

private class RecordingWsInbox : WsInbox(NoHandle) {
    val events = LinkedBlockingQueue<String>()

    override fun onOpen(id: String, acceptedProtocol: String?) {
        events.add("open:$id")
    }

    override fun onText(id: String, text: String) {
        events.add("text:$id:$text")
    }

    override fun onBinary(id: String, bytes: ByteArray) {
        events.add("binary:$id:${bytes.joinToString(",")}")
    }

    override fun onClosed(id: String, code: UShort?, reason: String) {
        events.add("closed:$id:${code ?: "?"}:$reason")
    }
}

private const val WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

private class SilentPeer : AutoCloseable {
    private val socket = ServerSocket(0, 1, InetAddress.getByName(LOOPBACK))
    val port: Int = socket.localPort

    init {
        thread(isDaemon = true) {
            val peer = runCatching { socket.accept() }.getOrNull() ?: return@thread
            val reader = peer.getInputStream().bufferedReader()
            val key = generateSequence { reader.readLine() }
                .takeWhile { it.isNotEmpty() }
                .firstNotNullOfOrNull { line ->
                    line.takeIf { it.startsWith("Sec-WebSocket-Key:", ignoreCase = true) }?.substringAfter(':')?.trim()
                } ?: return@thread
            val accept = Base64.getEncoder()
                .encodeToString(MessageDigest.getInstance("SHA-1").digest((key + WS_GUID).toByteArray()))
            peer.getOutputStream().write(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: $accept\r\n\r\n"
                    .toByteArray(),
            )
            peer.getOutputStream().flush()
            runCatching { while (peer.getInputStream().read() >= 0) Unit }
        }
    }

    override fun close() {
        socket.close()
    }
}

class KtorWsTransportTest {
    private fun connect(id: String, port: Int) =
        WsConnect(id = id, url = "ws://$LOOPBACK:$port/ws", protocols = emptyList(), headers = emptyList())

    private fun server(handler: suspend DefaultWebSocketServerSession.() -> Unit) = loopbackServer {
        install(WebSockets)
        routing { webSocket("/ws", handler = handler) }
    }

    @Test
    fun opensPumpsBothDirectionsAndReportsServerClose() {
        val received = LinkedBlockingQueue<String>()
        val (srv, port) = server {
            send(Frame.Text("hello"))
            for (frame in incoming) {
                if (frame is Frame.Text) {
                    received.add(frame.readText())
                    close(CloseReason(CloseReason.Codes.NORMAL, "done"))
                }
            }
        }
        try {
            KtorWsTransport().use { transport ->
                val inbox = RecordingWsInbox()
                transport.connect(connect("a", port), inbox)
                assertEquals("open:a", inbox.events.poll(5, TimeUnit.SECONDS))
                assertEquals("text:a:hello", inbox.events.poll(5, TimeUnit.SECONDS))
                transport.send("a", WsFrame.Text("""{"type":"pong"}"""))
                assertEquals("""{"type":"pong"}""", received.poll(5, TimeUnit.SECONDS))
                assertEquals("closed:a:1000:done", inbox.events.poll(5, TimeUnit.SECONDS))
            }
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun twoConnectionsAreIndependent() {
        val (srv, port) = server {
            for (frame in incoming) {
                if (frame is Frame.Text) send(Frame.Text("echo:${frame.readText()}"))
            }
        }
        try {
            KtorWsTransport().use { transport ->
                val a = RecordingWsInbox()
                val b = RecordingWsInbox()
                transport.connect(connect("a", port), a)
                transport.connect(connect("b", port), b)
                assertEquals("open:a", a.events.poll(5, TimeUnit.SECONDS))
                assertEquals("open:b", b.events.poll(5, TimeUnit.SECONDS))
                transport.send("a", WsFrame.Text("one"))
                transport.send("b", WsFrame.Text("two"))
                assertEquals("text:a:echo:one", a.events.poll(5, TimeUnit.SECONDS))
                assertEquals("text:b:echo:two", b.events.poll(5, TimeUnit.SECONDS))
            }
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun disconnectSendsTheCloseAndReportsIt() {
        val (srv, port) = server {
            while (incoming.receiveCatching().isSuccess) Unit
        }
        try {
            KtorWsTransport().use { transport ->
                val inbox = RecordingWsInbox()
                transport.connect(connect("a", port), inbox)
                assertEquals("open:a", inbox.events.poll(5, TimeUnit.SECONDS))
                transport.disconnect("a", 1001u, "going away")
                assertEquals("closed:a:1001:going away", inbox.events.poll(5, TimeUnit.SECONDS))
            }
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun connectFailureSurfacesAsClosed() {
        val port = closedLoopbackPort()
        KtorWsTransport().use { transport ->
            val inbox = RecordingWsInbox()
            transport.connect(connect("a", port), inbox)
            val event = inbox.events.poll(10, TimeUnit.SECONDS)
            assertTrue(event != null && event.startsWith("closed:a:"), "expected closed, got $event")
        }
    }

    @Test
    fun aReplacedSessionGoesSilent() {
        val (srv, port) = server {
            while (incoming.receiveCatching().isSuccess) Unit
        }
        try {
            KtorWsTransport().use { transport ->
                val first = RecordingWsInbox()
                transport.connect(connect("a", port), first)
                assertEquals("open:a", first.events.poll(5, TimeUnit.SECONDS))
                val second = RecordingWsInbox()
                transport.connect(connect("a", port), second)
                assertEquals("open:a", second.events.poll(5, TimeUnit.SECONDS))
                assertNull(first.events.poll(500, TimeUnit.MILLISECONDS))
            }
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun binaryFramesCrossIntact() {
        val (srv, port) = server {
            send(Frame.Binary(true, byteArrayOf(1, 2, 3)))
            while (incoming.receiveCatching().isSuccess) Unit
        }
        try {
            KtorWsTransport().use { transport ->
                val inbox = RecordingWsInbox()
                transport.connect(connect("a", port), inbox)
                assertEquals("open:a", inbox.events.poll(5, TimeUnit.SECONDS))
                assertEquals("binary:a:1,2,3", inbox.events.poll(5, TimeUnit.SECONDS))
            }
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun aPeerThatStopsAnsweringWithoutClosingIsReportedClosed() {
        SilentPeer().use { peer ->
            KtorWsTransport(pingInterval = 200.milliseconds).use { transport ->
                val inbox = RecordingWsInbox()
                transport.connect(connect("a", peer.port), inbox)
                assertEquals("open:a", inbox.events.poll(5, TimeUnit.SECONDS))
                val event = inbox.events.poll(5, TimeUnit.SECONDS)
                assertTrue(
                    event != null && event.startsWith("closed:a:"),
                    "a socket whose far end went silent without a close is dead and has to be reported, got $event",
                )
            }
        }
    }

    @Test
    fun aPeerThatAnswersPingsStaysOpen() {
        val (srv, port) = server {
            delay(2_000)
            send(Frame.Text("still here"))
            while (incoming.receiveCatching().isSuccess) Unit
        }
        try {
            KtorWsTransport(pingInterval = 200.milliseconds).use { transport ->
                val inbox = RecordingWsInbox()
                transport.connect(connect("a", port), inbox)
                assertEquals("open:a", inbox.events.poll(5, TimeUnit.SECONDS))
                assertEquals("text:a:still here", inbox.events.poll(5, TimeUnit.SECONDS))
            }
        } finally {
            srv.stop(0, 0)
        }
    }
}
