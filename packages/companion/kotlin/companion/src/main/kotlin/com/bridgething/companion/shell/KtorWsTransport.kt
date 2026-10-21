package com.bridgething.companion.shell

import io.ktor.client.HttpClient
import io.ktor.client.engine.cio.CIO
import io.ktor.client.plugins.websocket.WebSockets
import io.ktor.client.plugins.websocket.webSocketSession
import io.ktor.client.request.header
import io.ktor.http.HttpHeaders
import io.ktor.http.takeFrom
import io.ktor.websocket.CloseReason
import io.ktor.websocket.Frame
import io.ktor.websocket.close
import io.ktor.websocket.readBytes
import io.ktor.websocket.readText
import java.util.concurrent.ConcurrentHashMap
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import uniffi.bridgething_companion.WsConnect
import uniffi.bridgething_companion.WsFrame
import uniffi.bridgething_companion.WsInbox
import uniffi.bridgething_companion.WsTransport

private const val WS_ABNORMAL_CLOSURE: UShort = 1006u

public class KtorWsTransport(private val pingInterval: Duration = 3.seconds) : WsTransport, AutoCloseable {
    private val lock = Any()
    private var scope: CoroutineScope? = null
    private var client: HttpClient? = null

    private fun liveScope(): CoroutineScope = synchronized(lock) {
        scope ?: CoroutineScope(SupervisorJob() + Dispatchers.IO).also { scope = it }
    }

    private fun liveClient(): HttpClient = synchronized(lock) {
        client ?: HttpClient(CIO) {
            install(WebSockets) {
                pingIntervalMillis = pingInterval.inWholeMilliseconds
            }
            engine {
                requestTimeout = 0
            }
        }.also { client = it }
    }

    override fun close() {
        conns.keys.toList().forEach { conns.remove(it)?.shutdown() }
        val (deadScope, deadClient) = synchronized(lock) {
            val held = scope to client
            scope = null
            client = null
            held
        }
        deadScope?.cancel()
        deadClient?.close()
    }

    private sealed interface Outgoing {
        data class Send(val frame: WsFrame) : Outgoing

        data class Close(val code: UShort?, val reason: String?) : Outgoing
    }

    private class Conn(val outbound: Channel<Outgoing>, val job: Job) {
        fun shutdown() {
            job.cancel()
            outbound.close()
        }
    }

    private val conns = ConcurrentHashMap<String, Conn>()

    override fun connect(connect: WsConnect, inbox: WsInbox) {
        val outbound = Channel<Outgoing>(Channel.UNLIMITED)
        lateinit var conn: Conn
        val job = liveScope().launch(start = CoroutineStart.LAZY) {
            try {
                pump(connect, inbox, outbound)
            } finally {
                conns.remove(connect.id, conn)
                inbox.close()
            }
        }
        conn = Conn(outbound, job)
        conns.put(connect.id, conn)?.shutdown()
        job.start()
    }

    override fun send(id: String, frame: WsFrame) {
        conns[id]?.outbound?.trySend(Outgoing.Send(frame))
    }

    override fun disconnect(id: String, code: UShort?, reason: String?) {
        conns[id]?.outbound?.trySend(Outgoing.Close(code, reason))
    }

    private suspend fun pump(connect: WsConnect, inbox: WsInbox, outbound: Channel<Outgoing>) {
        val id = connect.id
        val session = try {
            liveClient().webSocketSession {
                url.takeFrom(connect.url)
                for (h in connect.headers) header(h.name, h.value)
                if (connect.protocols.isNotEmpty()) {
                    header(HttpHeaders.SecWebSocketProtocol, connect.protocols.joinToString(", "))
                }
            }
        } catch (c: CancellationException) {
            throw c
        } catch (t: Throwable) {
            inbox.onClosed(id, null, "connect failed: ${t.message ?: t}")
            return
        }

        val acceptedProtocol = session.call.response.headers[HttpHeaders.SecWebSocketProtocol]
        inbox.onOpen(id, acceptedProtocol)

        var reported = false
        try {
            coroutineScope {
                val writer = launch {
                    for (out in outbound) {
                        when (out) {
                            is Outgoing.Send -> when (val frame = out.frame) {
                                is WsFrame.Text -> session.send(Frame.Text(frame.text))
                                is WsFrame.Binary -> session.send(Frame.Binary(true, frame.bytes))
                            }
                            is Outgoing.Close -> {
                                val code = out.code ?: 1000u
                                runCatching { session.close(CloseReason(code.toShort(), out.reason ?: "")) }
                                inbox.onClosed(id, code, out.reason ?: "")
                                reported = true
                                this@coroutineScope.cancel()
                            }
                        }
                    }
                }
                for (frame in session.incoming) {
                    when (frame) {
                        is Frame.Text -> inbox.onText(id, frame.readText())
                        is Frame.Binary -> inbox.onBinary(id, frame.readBytes())
                        else -> {}
                    }
                }
                val reason = session.closeReason.await()
                inbox.onClosed(
                    id,
                    reason?.code?.toUShort() ?: WS_ABNORMAL_CLOSURE,
                    reason?.message?.takeIf { it.isNotEmpty() } ?: "closed",
                )
                reported = true
                writer.cancel()
            }
        } catch (c: CancellationException) {
            throw c
        } catch (t: Throwable) {
            if (!reported) inbox.onClosed(id, WS_ABNORMAL_CLOSURE, "read error: ${t.message ?: t}")
        } finally {
            session.cancel()
        }
    }
}
