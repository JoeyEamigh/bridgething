package com.bridgething.companion.shell

import io.ktor.server.cio.CIO
import io.ktor.server.engine.EmbeddedServer
import io.ktor.server.engine.embeddedServer
import io.ktor.server.request.receive
import io.ktor.server.response.respondBytes
import io.ktor.server.response.respondText
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.routing.route
import io.ktor.server.routing.routing
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import uniffi.bridgething_companion.HttpDownloadSink
import uniffi.bridgething_companion.HttpHeader
import uniffi.bridgething_companion.HttpMethod
import uniffi.bridgething_companion.HttpRequest
import uniffi.bridgething_companion.HttpResponse
import uniffi.bridgething_companion.HttpSink
import uniffi.bridgething_companion.NoHandle

private class RecordingHttpSink : HttpSink(NoHandle) {
    val outcomes = LinkedBlockingQueue<Result<HttpResponse>>()

    override fun complete(response: HttpResponse) {
        outcomes.add(Result.success(response))
    }

    override fun fail(reason: String) {
        outcomes.add(Result.failure(Exception(reason)))
    }
}

private class RecordingDownloadSink : HttpDownloadSink(NoHandle) {
    val events = LinkedBlockingQueue<String>()
    val chunks = mutableListOf<ByteArray>()

    override fun onResponse(status: UShort, headers: List<HttpHeader>, contentLength: ULong?) {
        events.add("response:$status:${contentLength ?: "?"}")
    }

    override fun onChunk(chunk: ByteArray) {
        synchronized(chunks) { chunks.add(chunk) }
    }

    override fun onFinished() {
        events.add("finished")
    }

    override fun onFailed(reason: String) {
        events.add("failed:$reason")
    }
}

class KtorHttpTransportTest {
    private fun request(url: String, method: HttpMethod = HttpMethod.Get, body: ByteArray = ByteArray(0)) =
        HttpRequest(method = method, url = url, headers = emptyList(), body = body, timeoutMs = 5_000u)

    private fun server(build: io.ktor.server.routing.Routing.() -> Unit): Pair<EmbeddedServer<*, *>, Int> {
        val srv = embeddedServer(CIO, port = 0) { routing { build() } }.start(wait = false)
        val port = runBlocking { srv.engine.resolvedConnectors().first().port }
        return srv to port
    }

    @Test
    fun executeRoundTripsStatusHeadersAndBody() {
        val (srv, port) = server {
            get("/hello") {
                call.response.headers.append("x-thing", "yes")
                call.respondText("hi there")
            }
        }
        try {
            val sink = RecordingHttpSink()
            KtorHttpTransport().execute(request("http://127.0.0.1:$port/hello"), sink)
            val resp = sink.outcomes.poll(5, TimeUnit.SECONDS)!!.getOrThrow()
            assertEquals(200.toUShort(), resp.status)
            assertEquals("hi there", resp.body.decodeToString())
            assertEquals("yes", resp.headers.firstOrNull { it.name.equals("x-thing", true) }?.value)
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun executeSendsTheBodyAndFailsOnlyThroughTheSink() {
        val received = LinkedBlockingQueue<ByteArray>()
        val (srv, port) = server {
            post("/echo") {
                received.add(call.receive<ByteArray>())
                call.respondText("ok")
            }
        }
        try {
            val sink = RecordingHttpSink()
            KtorHttpTransport().execute(
                request("http://127.0.0.1:$port/echo", HttpMethod.Post, byteArrayOf(4, 5, 6)),
                sink,
            )
            assertTrue(sink.outcomes.poll(5, TimeUnit.SECONDS)!!.isSuccess)
            assertArrayEquals(byteArrayOf(4, 5, 6), received.poll(1, TimeUnit.SECONDS))
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun aVerbTheEnumDoesNotNameReachesTheServerVerbatim() {
        val seen = LinkedBlockingQueue<String>()
        val (srv, port) = server {
            route("/dav", io.ktor.http.HttpMethod.parse("PROPFIND")) {
                handle {
                    seen.add(call.request.local.method.value)
                    call.respondText("ok")
                }
            }
        }
        try {
            val sink = RecordingHttpSink()
            KtorHttpTransport().execute(
                request("http://127.0.0.1:$port/dav", HttpMethod.Other("PROPFIND")),
                sink,
            )
            assertTrue(sink.outcomes.poll(5, TimeUnit.SECONDS)!!.isSuccess)
            assertEquals("PROPFIND", seen.poll(1, TimeUnit.SECONDS))
        } finally {
            srv.stop(0, 0)
        }
    }

    @Test
    fun aConnectFailureLandsAsFailNeverAHang() {
        val port = java.net.ServerSocket(0).use { it.localPort }
        val sink = RecordingHttpSink()
        KtorHttpTransport().execute(request("http://127.0.0.1:$port/nope"), sink)
        val outcome = sink.outcomes.poll(10, TimeUnit.SECONDS)
        assertTrue(outcome != null && outcome.isFailure, "expected a fail, got $outcome")
    }

    @Test
    fun downloadStreamsChunksAndFinishes() {
        val payload = ByteArray(256 * 1024) { (it % 251).toByte() }
        val (srv, port) = server {
            get("/artifact") { call.respondBytes(payload) }
        }
        try {
            val sink = RecordingDownloadSink()
            KtorHttpTransport().download(request("http://127.0.0.1:$port/artifact"), sink)
            assertEquals("response:200:${payload.size}", sink.events.poll(5, TimeUnit.SECONDS))
            assertEquals("finished", sink.events.poll(5, TimeUnit.SECONDS))
            val whole = synchronized(sink.chunks) {
                sink.chunks.fold(ByteArray(0)) { acc, chunk -> acc + chunk }
            }
            assertArrayEquals(payload, whole)
        } finally {
            srv.stop(0, 0)
        }
    }
}
