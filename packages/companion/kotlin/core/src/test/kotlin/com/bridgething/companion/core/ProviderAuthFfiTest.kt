package com.bridgething.companion.core

import java.nio.file.Files
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import uniffi.bridgething_companion.AuthKind
import uniffi.bridgething_companion.CapabilityFlags
import uniffi.bridgething_companion.CompanionBackends
import uniffi.bridgething_companion.CompanionConfig
import uniffi.bridgething_companion.CompanionException
import uniffi.bridgething_companion.CompanionSession
import uniffi.bridgething_companion.HostClock
import uniffi.bridgething_companion.HostEnvironment
import uniffi.bridgething_companion.HostInfo
import uniffi.bridgething_companion.HttpDownloadSink
import uniffi.bridgething_companion.HttpRequest
import uniffi.bridgething_companion.HttpResponse
import uniffi.bridgething_companion.HttpSink
import uniffi.bridgething_companion.HttpTransport
import uniffi.bridgething_companion.LogLevel
import uniffi.bridgething_companion.LogSink
import uniffi.bridgething_companion.ProviderCredentials
import uniffi.bridgething_companion.SecretStore
import uniffi.bridgething_companion.SessionEvent
import uniffi.bridgething_companion.SessionEventSink
import uniffi.bridgething_companion.SpotifyProviderConfig
import uniffi.bridgething_companion.WsConnect
import uniffi.bridgething_companion.WsFrame
import uniffi.bridgething_companion.WsInbox
import uniffi.bridgething_companion.WsTransport

private const val AWAIT_MS = 15_000L
private const val WORKER_BASE = "https://worker.test/auth"
private const val REFRESH_KEY = "spotify.refresh_token"

private class MapSecretStore : SecretStore {
  val values = ConcurrentHashMap<String, String>()

  override fun get(key: String): String? = values[key]

  override fun set(key: String, value: String) {
    values[key] = value
  }

  override fun remove(key: String) {
    values.remove(key)
  }

  override fun getBlob(key: String): ByteArray? = null
}

private class FakeWorkerHttp : HttpTransport {
  override fun execute(request: HttpRequest, sink: HttpSink) {
    sink.use {
      val body = request.body.toString(Charsets.UTF_8)
      when {
        request.url == "$WORKER_BASE/api/device/code" -> it.complete(
          json(
            """{"device_code":"dc-1","user_code":"WXYZ","verification_url":"https://spotify.com/pair",""" +
              """"interval":1,"expires_in":600}""",
          ),
        )
        request.url == "$WORKER_BASE/api/token" && body.contains("device_code") -> it.complete(
          json("""{"access_token":"worker-bearer","refresh_token":"worker-refresh","expires_in":3600}"""),
        )
        request.url == "$WORKER_BASE/api/token" && body.contains("refresh_token") -> it.complete(
          json("""{"access_token":"refreshed-bearer","expires_in":3600}"""),
        )
        else -> it.fail("the fake worker only speaks auth: ${request.url}")
      }
    }
  }

  override fun download(request: HttpRequest, sink: HttpDownloadSink) {
    sink.use { it.onFailed("the fake worker serves no downloads") }
  }

  private fun json(body: String) = HttpResponse(status = 200u, headers = emptyList(), body = body.toByteArray())
}

private class DeadWs : WsTransport {
  override fun connect(connect: WsConnect, inbox: WsInbox) {
    inbox.use { it.onClosed(connect.id, null, "the auth test has no dealer") }
  }

  override fun send(id: String, frame: WsFrame) {}

  override fun disconnect(id: String, code: UShort?, reason: String?) {}
}

private class WorkerTestHost : HostEnvironment {
  override fun clock(): HostClock =
    HostClock(tzIana = "UTC", locale = "en-US", unixSeconds = 1_700_000_000uL, utcOffsetMinutes = 0, dstOffsetMinutes = 0)
}

private class QuietLog : LogSink {
  override fun onLine(level: LogLevel, target: String, message: String) {}
}

private class EventQueue : SessionEventSink {
  val events = LinkedBlockingQueue<SessionEvent>()

  override fun onEvent(event: SessionEvent) {
    events.add(event)
  }

  fun awaitAuthKind(wanted: AuthKind): Boolean {
    val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(AWAIT_MS)
    while (System.nanoTime() < deadline) {
      val event = events.poll(200, TimeUnit.MILLISECONDS) ?: continue
      if (event is SessionEvent.ProvidersChanged &&
        event.providers.any { it.id == "spotify" && it.authState.kind == wanted }
      ) {
        return true
      }
    }
    return false
  }
}

private fun session(secrets: SecretStore, events: EventQueue): CompanionSession {
  val scratch = Files.createTempDirectory("companion-auth").toFile().absolutePath
  return CompanionSession.create(
    config = CompanionConfig(
      host = HostInfo(
        appName = "auth-test",
        appVersion = "0.0.0",
        osName = "linux",
        osVersion = "test",
        hostIdentifier = "jvm-auth",
      ),
      capabilities = CapabilityFlags(
        geo = false,
        notifications = false,
        netFetch = false,
        netWs = false,
        audioTts = false,
        voiceModel = false,
      ),
      stateDir = scratch,
      cacheDir = scratch,
      spotify = SpotifyProviderConfig(workerBase = WORKER_BASE, psk = "test-psk"),
    ),
    backends = CompanionBackends(
      link = null,
      host = WorkerTestHost(),
      http = FakeWorkerHttp(),
      ws = DeadWs(),
      secrets = secrets,
      log = QuietLog(),
    ),
    events = events,
  )
}

class ProviderAuthFfiTest {
  @Test
  fun theCatalogListsSpotifyBeforeAnythingIsConnected() =
    runBlocking {
      val session = session(MapSecretStore(), EventQueue())
      val providers = withTimeout(AWAIT_MS) { session.availableProviders() }
      assertEquals(listOf("spotify"), providers.map { it.id })
      assertTrue(providers[0].available)
      assertFalse(providers[0].connected)
      assertEquals(AuthKind.IDLE, providers[0].authState.kind)
    }

  @Test
  fun anUnknownProviderIdIsRefused(): Unit =
    runBlocking {
      val session = session(MapSecretStore(), EventQueue())
      assertThrows<CompanionException> { withTimeout(AWAIT_MS) { session.connectProvider("tidal") } }
    }

  @Test
  fun connectRunsTheDeviceFlowThroughPendingToAuthenticated() =
    runBlocking {
      val secrets = MapSecretStore()
      val events = EventQueue()
      val session = session(secrets, events)

      withTimeout(AWAIT_MS) { session.connectProvider("spotify") }

      assertTrue(events.awaitAuthKind(AuthKind.PENDING), "the sign-in conversation surfaced a pending state")
      assertTrue(events.awaitAuthKind(AuthKind.AUTHENTICATED), "the approved device flow authenticated")
      assertEquals("worker-refresh", secrets.values[REFRESH_KEY], "the worker's refresh token was persisted")
      val provider = withTimeout(AWAIT_MS) { session.availableProviders() }.first()
      assertTrue(provider.connected)
    }

  @Test
  fun completingAPkceSignInPersistsThePairAndConnects() =
    runBlocking {
      val secrets = MapSecretStore()
      val events = EventQueue()
      val session = session(secrets, events)

      withTimeout(AWAIT_MS) {
        session.completeProviderAuth(
          "spotify",
          ProviderCredentials.OauthTokens(accessToken = "pkce-bearer", refreshToken = "pkce-refresh"),
        )
      }

      assertEquals("pkce-refresh", secrets.values[REFRESH_KEY], "the handed-down refresh token was persisted")
      assertTrue(events.awaitAuthKind(AuthKind.AUTHENTICATED), "the stored pair signed the provider in silently")
      assertTrue(withTimeout(AWAIT_MS) { session.availableProviders() }.first().connected)
    }

  @Test
  fun disconnectSignsOutAndCancelKeepsCredentials() =
    runBlocking {
      val secrets = MapSecretStore()
      val events = EventQueue()
      val session = session(secrets, events)
      withTimeout(AWAIT_MS) {
        session.completeProviderAuth(
          "spotify",
          ProviderCredentials.OauthTokens(accessToken = "pkce-bearer", refreshToken = "pkce-refresh"),
        )
      }
      assertTrue(events.awaitAuthKind(AuthKind.AUTHENTICATED))

      withTimeout(AWAIT_MS) { session.cancelAuth("spotify") }
      assertEquals("pkce-refresh", secrets.values[REFRESH_KEY], "cancel leaves the stored sign-in alone")
      assertFalse(withTimeout(AWAIT_MS) { session.availableProviders() }.first().connected)

      withTimeout(AWAIT_MS) { session.connectProvider("spotify") }
      withTimeout(AWAIT_MS) { session.disconnectProvider("spotify") }
      assertNull(secrets.values[REFRESH_KEY], "disconnect is a sign-out and clears the credentials")
      assertFalse(withTimeout(AWAIT_MS) { session.availableProviders() }.first().connected)
    }

  @Test
  fun storedCredentialsRestoreTheSignInOnStart() =
    runBlocking {
      val secrets = MapSecretStore()
      secrets.values[REFRESH_KEY] = "stored-refresh"
      val events = EventQueue()
      val session = session(secrets, events)

      withTimeout(AWAIT_MS) { session.start() }

      assertTrue(events.awaitAuthKind(AuthKind.AUTHENTICATED), "an install holding a refresh token comes back signed in")
      assertTrue(withTimeout(AWAIT_MS) { session.availableProviders() }.first().connected)
      withTimeout(AWAIT_MS) { session.stop() }
    }
}
