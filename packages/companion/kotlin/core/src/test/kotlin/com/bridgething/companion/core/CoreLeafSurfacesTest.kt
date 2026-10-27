package com.bridgething.companion.core

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import uniffi.bridgething_companion.NluFastPathRepeatMode
import uniffi.bridgething_companion.NluRejectionOutcome
import uniffi.bridgething_companion.NluRejectionPolicy
import uniffi.bridgething_companion.OtaKind
import uniffi.bridgething_companion.OtaLane
import uniffi.bridgething_companion.OtaPhaseSnapshot
import uniffi.bridgething_companion.OtaPlanStep
import uniffi.bridgething_companion.OtaPollEvent
import uniffi.bridgething_companion.OtaRunOutcome
import uniffi.bridgething_companion.OtaRunStore
import uniffi.bridgething_companion.OtaStepKind
import uniffi.bridgething_companion.OtaStoreChange
import uniffi.bridgething_companion.nluFastPathMatch
import uniffi.bridgething_companion.nluIntentCatalog
import uniffi.bridgething_companion.nluRejectionEvaluate
import uniffi.bridgething_companion.otaArtifactUrls
import uniffi.bridgething_companion.otaCompositeVersionString
import uniffi.bridgething_companion.otaRunProgress
import uniffi.bridgething_companion.parseLrc
import uniffi.bridgething_companion.parseOtaCompositeVersion
import uniffi.bridgething_companion.parseOtaDiscoverManifest

class CoreLeafSurfacesTest {
  @Test
  fun lrcLinesComeBackSortedWithTimestamps() {
    val lines = parseLrc("[00:12.00]world\n[00:01.50]hello\n")
    assertEquals(listOf(1_500u to "hello", 12_000u to "world"), lines.map { it.startMs to it.text })
  }

  @Test
  fun fastPathHitCarriesTypedSlots() {
    val hit = requireNotNull(nluFastPathMatch("repeat off"))
    assertEquals("SET_REPEAT", hit.intent)
    assertEquals(NluFastPathRepeatMode.OFF, hit.slots.repeatMode)
    assertNull(nluFastPathMatch("play some norwegian jazz"))
  }

  @Test
  fun rejectionAcceptsAConfidentHead() {
    val catalog = nluIntentCatalog()
    assertEquals(22, catalog.surfaceNames.size)
    val logits = MutableList(catalog.surfaceNames.size) { -8.0 }
    logits[catalog.surfaceNames.indexOf("PAUSE")] = 8.0
    val outcome = nluRejectionEvaluate(logits, 6.0, NluRejectionPolicy())
    assertEquals(NluRejectionOutcome.Accept("PAUSE"), outcome)
  }

  @Test
  fun manifestRoundTripsThroughTheCoreParser() {
    val manifest = parseOtaDiscoverManifest(
      """
      {
        "manifest_version": 1,
        "updated_at": "2026-08-01T00:00:00Z",
        "channels": {
          "stable": {"name": "Stable", "stability": "stable", "default": true, "latest": "1.2.3+image.4.5.6", "releases": ["1.2.3+image.4.5.6"]}
        },
        "releases": {
          "1.2.3+image.4.5.6": {"version": "1.2.3+image.4.5.6", "channel": "stable", "yanked": null, "deprecated": false}
        }
      }
      """.trimIndent(),
    )
    val latest = manifest.channels.getValue("stable").latest
    val composite = requireNotNull(parseOtaCompositeVersion(latest))
    assertEquals("1.2.3", composite.daemon)
    assertEquals(latest, otaCompositeVersionString(composite))
    val urls = otaArtifactUrls("https://ota.bridgething.com/", "stable", composite.daemon, composite.image, "prod")
    assertEquals("https://ota.bridgething.com/daemon/stable/1.2.3/bridgething", urls.daemonBinary)
  }

  @Test
  fun runStoreReducesAPlanThroughToADismissableSuccess() {
    OtaRunStore().use { store ->
      val steps = listOf(OtaPlanStep(0u, OtaStepKind.DOWNLOAD, "update.swu", 1_000uL))
      store.ingest(
        OtaPollEvent.Planned(
          "dev-1", OtaKind.IMAGE, "1+image.2", "1", "2",
          "stable", "https://ota.bridgething.com", steps,
        ),
      )
      store.ingest(
        OtaPollEvent.Progress(
          "dev-1", OtaKind.IMAGE, 0u,
          OtaPhaseSnapshot.Downloading("update.swu", 500uL, 1_000uL, null),
        ),
      )
      val run = store.runs().single()
      assertEquals(OtaLane.FIRMWARE, run.lane)
      val progress = otaRunProgress(run, run.phaseStartedAtMs)
      assertTrue(progress.percent in 1u..99u) { "mid-download percent, got ${progress.percent}" }
      assertNull(store.dismiss("dev-1", OtaLane.FIRMWARE)) { "an unfinished run must refuse dismissal" }

      val changes = store.ingest(OtaPollEvent.Updated("dev-1", OtaKind.IMAGE, "1+image.2"))
      assertTrue(changes.any { it is OtaStoreChange.Run && it.run.outcome == OtaRunOutcome.SUCCEEDED })
      assertNotNull(store.dismiss("dev-1", OtaLane.FIRMWARE))
    }
  }
}
