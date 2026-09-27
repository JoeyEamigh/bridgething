package com.bridgething.companion

import android.content.ComponentName
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URI
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.bridgething_companion.CapabilityFlags
import uniffi.bridgething_companion.HostInfo
import uniffi.bridgething_companion.LinkDevice
import uniffi.bridgething_companion.PeerLinkStatus
import uniffi.bridgething_companion.SessionEvent
import uniffi.bridgething_companion.WebappInstallRequest
import uniffi.bridgething_companion.WebappRole
import uniffi.bridgething_companion.WebappSlot

@RunWith(AndroidJUnit4::class)
class SessionDevLaneTest {
    private lateinit var companion: BridgethingCompanion
    private val events = LinkedBlockingQueue<SessionEvent>()
    private val seen = mutableListOf<SessionEvent>()

    @Before
    fun dial() {
        assumeTrue("no dev daemon reachable at $GATEWAY_URL", reachable(GATEWAY_URL))
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        companion = BridgethingCompanion(
            context = context,
            host = HostInfo(
                appName = "bridgething-androidtest",
                appVersion = "0.0.0",
                osName = "Android",
                osVersion = android.os.Build.VERSION.RELEASE ?: "",
                hostIdentifier = "androidtest",
            ),
            capabilities = CapabilityFlags(
                geo = true,
                notifications = true,
                netFetch = true,
                netWs = true,
                audioTts = true,
                voiceModel = false,
            ),
            resolveNotificationAction = { _, _ -> null },
            notificationListener = ComponentName(context, TestNotificationListener::class.java),
            spotify = null,
            events = { event -> events.add(event) },
        )
        runBlocking {
            companion.start()
            companion.session.connectNetwork(GATEWAY_URL, LinkDevice(id = DEVICE_ID, name = "dev gateway"))
        }
        awaitEvent<SessionEvent.PeerConnected> { it.peer.id == DEVICE_ID }
    }

    @After
    fun hangUp() {
        if (!this::companion.isInitialized) return
        runBlocking {
            companion.session.disconnectNetwork(DEVICE_ID)
            companion.stop()
        }
    }

    @Test
    fun theDialedGatewayBecomesTheSessionPeer() {
        val snapshot = runBlocking { companion.snapshot() }
        val peer = snapshot.peers.firstOrNull { it.id == DEVICE_ID }
        assertNotNull("the dialed gateway is not in the snapshot", peer)
        assertEquals(PeerLinkStatus.CONNECTED, peer!!.status)

        val meta = awaitEvent<SessionEvent.DeviceMetaChanged> { it.deviceId == DEVICE_ID }
        assertTrue("the daemon reported no version", meta.meta.daemonVersion.isNotEmpty())
        assertTrue(
            "the announced capabilities did not survive the round trip",
            snapshot.capabilityFlags.netFetch && snapshot.capabilityFlags.netWs,
        )
    }

    @Test
    fun theWebappCatalogueReadsBackOverTheLink() {
        val webapps = runBlocking { companion.session.listWebapps(DEVICE_ID) }
        assertTrue("the daemon serves no webapps", webapps.isNotEmpty())

        val slots = runBlocking { companion.session.webappSlots(DEVICE_ID) }
        val launcher = webapps.firstOrNull { it.role == WebappRole.LAUNCHER }?.id ?: slots.launcher
        forget()
        val written = runBlocking { companion.session.setWebappSlot(DEVICE_ID, WebappSlot.LAUNCHER, launcher) }
        assertEquals("the launcher slot did not read back", launcher, written.launcher)
        awaitEvent<SessionEvent.WebappsChanged> { it.entry.deviceId == DEVICE_ID }
    }

    @Test
    fun aWebappDocRoundTripsThroughTheDevice() {
        val target = runBlocking { companion.session.listWebapps(DEVICE_ID) }.first()
        val value = "androidtest-${System.currentTimeMillis()}"
        runBlocking {
            companion.session.setWebappDoc(DEVICE_ID, target.id, DOC_KEY, value)
            assertEquals(value, companion.session.getWebappDoc(DEVICE_ID, target.id, DOC_KEY))
            companion.session.deleteWebappDoc(DEVICE_ID, target.id, DOC_KEY)
            assertEquals(null, companion.session.getWebappDoc(DEVICE_ID, target.id, DOC_KEY))
        }
    }

    @Test
    fun aPublishedBundleInstallsOverTheUpdateSurface() {
        val url = InstrumentationRegistry.getArguments().getString("bridgethingWebappUrl")
        assumeTrue("pass -e bridgethingWebappUrl <url> to run the install tier", url != null)
        val before = runBlocking { companion.session.listWebapps(DEVICE_ID) }.map { it.id }.toSet()

        val installed = runBlocking {
            companion.session.installWebappFromUrl(DEVICE_ID, WebappInstallRequest(url!!, null, url, null, null))
        }
        assertEquals(url, installed.provenance)
        awaitEvent<SessionEvent.WebappsChanged> { it.entry.webapps.any { app -> app.id == installed.id } }

        if (installed.id !in before) {
            runBlocking { companion.session.uninstallWebapp(DEVICE_ID, installed.id) }
        }
    }

    @Test
    fun theOtaManifestFetchesThroughTheHostHttpSeam() {
        val root = InstrumentationRegistry.getArguments().getString("bridgethingOtaRoot")
        assumeTrue("pass -e bridgethingOtaRoot <url> to run the ota tier", root != null)
        val manifest = runBlocking { companion.session.fetchOtaManifest(root!!) }
        assertTrue("the manifest names no channels", manifest.channels.isNotEmpty())

        runBlocking { companion.session.checkForOtaUpdate(root!!) }
        awaitEvent<SessionEvent.OtaPollChanged> { it.status.lastPolledAt != null }
    }

    private inline fun <reified T : SessionEvent> awaitEvent(predicate: (T) -> Boolean): T {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(EVENT_TIMEOUT_SECONDS)
        while (true) {
            events.drainTo(seen)
            seen.filterIsInstance<T>().firstOrNull(predicate)?.let { return it }
            val remaining = deadline - System.nanoTime()
            if (remaining <= 0) break
            seen.add(events.poll(remaining, TimeUnit.NANOSECONDS) ?: break)
        }
        throw AssertionError("no ${T::class.simpleName} matched within ${EVENT_TIMEOUT_SECONDS}s; saw $seen")
    }

    private fun forget() {
        events.clear()
        seen.clear()
    }

    private companion object {
        const val DEVICE_ID = "dev-gateway"
        const val DOC_KEY = "androidtest.doc"
        const val EVENT_TIMEOUT_SECONDS = 20L

        val GATEWAY_URL: String =
            InstrumentationRegistry.getArguments().getString("bridgethingGateway") ?: "ws://10.0.2.2:8892/"

        fun reachable(url: String): Boolean {
            val uri = URI(url)
            val port = if (uri.port > 0) uri.port else 8892
            return runCatching {
                Socket().use { it.connect(InetSocketAddress(uri.host, port), 2000) }
            }.isSuccess
        }
    }
}
