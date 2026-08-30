package com.bridgething.companion.shell

import io.ktor.client.HttpClient
import io.ktor.client.engine.cio.CIO
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.HttpTimeoutConfig
import io.ktor.client.plugins.timeout
import io.ktor.client.request.header
import io.ktor.client.request.prepareRequest
import io.ktor.client.request.request
import io.ktor.client.request.setBody
import io.ktor.client.statement.HttpResponse as KtorResponse
import io.ktor.client.statement.bodyAsBytes
import io.ktor.client.statement.bodyAsChannel
import io.ktor.http.HttpMethod as KtorMethod
import io.ktor.utils.io.ByteReadChannel
import io.ktor.utils.io.readAvailable
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import uniffi.bridgething_companion.HttpDownloadSink
import uniffi.bridgething_companion.HttpHeader
import uniffi.bridgething_companion.HttpMethod
import uniffi.bridgething_companion.HttpRequest
import uniffi.bridgething_companion.HttpResponse
import uniffi.bridgething_companion.HttpSink
import uniffi.bridgething_companion.HttpTransport

public class KtorHttpTransport : HttpTransport {
    private val lock = Any()
    private var scope: CoroutineScope? = null
    private var client: HttpClient? = null

    private fun live(): Pair<CoroutineScope, HttpClient> = synchronized(lock) {
        val liveScope = scope ?: CoroutineScope(SupervisorJob() + Dispatchers.IO).also { scope = it }
        val liveClient = client ?: HttpClient(CIO) {
            expectSuccess = false
            install(HttpTimeout) {
                requestTimeoutMillis = 60_000
                connectTimeoutMillis = 15_000
            }
        }.also { client = it }
        liveScope to liveClient
    }

    public fun close() {
        val (deadScope, deadClient) = synchronized(lock) {
            val held = scope to client
            scope = null
            client = null
            held
        }
        deadScope?.cancel()
        deadClient?.close()
    }

    override fun execute(request: HttpRequest, sink: HttpSink) {
        val (scope, client) = live()
        scope.launch {
            sink.use {
                try {
                    val resp = client.request(request.url) { apply(request) }
                    it.complete(
                        HttpResponse(
                            status = resp.status.value.toUShort(),
                            headers = headersOf(resp),
                            body = resp.bodyAsBytes(),
                        ),
                    )
                } catch (t: Throwable) {
                    runCatching { it.fail(t.message ?: t.toString()) }
                }
            }
        }
    }

    override fun download(request: HttpRequest, sink: HttpDownloadSink) {
        val (scope, client) = live()
        scope.launch {
            sink.use { held ->
                try {
                    client.prepareRequest(request.url) {
                        apply(request)
                        timeout { requestTimeoutMillis = HttpTimeoutConfig.INFINITE_TIMEOUT_MS }
                    }.execute { resp ->
                        held.onResponse(
                            status = resp.status.value.toUShort(),
                            headers = headersOf(resp),
                            contentLength = resp.headers["Content-Length"]?.toLongOrNull()?.toULong(),
                        )
                        val channel: ByteReadChannel = resp.bodyAsChannel()
                        val buf = ByteArray(64 * 1024)
                        while (true) {
                            val read = channel.readAvailable(buf, 0, buf.size)
                            if (read < 0) break
                            if (read > 0) held.onChunk(buf.copyOf(read))
                        }
                        held.onFinished()
                    }
                } catch (t: Throwable) {
                    runCatching { held.onFailed(t.message ?: t.toString()) }
                }
            }
        }
    }

    private fun io.ktor.client.request.HttpRequestBuilder.apply(request: HttpRequest) {
        method = when (val verb = request.method) {
            is HttpMethod.Get -> KtorMethod.Get
            is HttpMethod.Head -> KtorMethod.Head
            is HttpMethod.Post -> KtorMethod.Post
            is HttpMethod.Put -> KtorMethod.Put
            is HttpMethod.Patch -> KtorMethod.Patch
            is HttpMethod.Delete -> KtorMethod.Delete
            is HttpMethod.Options -> KtorMethod.Options
            is HttpMethod.Other -> KtorMethod.parse(verb.verb)
        }
        for (h in request.headers) header(h.name, h.value)
        if (request.timeoutMs > 0u) {
            timeout { requestTimeoutMillis = request.timeoutMs.toLong() }
        }
        if (request.body.isNotEmpty()) setBody(request.body)
    }

    private fun headersOf(resp: KtorResponse): List<HttpHeader> = buildList {
        resp.headers.forEach { name, values ->
            for (v in values) add(HttpHeader(name, v))
        }
    }
}
