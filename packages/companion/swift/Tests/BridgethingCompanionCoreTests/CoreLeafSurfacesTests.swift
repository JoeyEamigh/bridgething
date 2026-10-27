import BridgethingCompanionCore
import XCTest

final class CoreLeafSurfacesTests: XCTestCase {
    func testLrcLinesComeBackSortedWithTimestamps() {
        let lines = parseLrc(text: "[00:12.00]world\n[00:01.50]hello\n")
        XCTAssertEqual(lines.map(\.startMs), [1500, 12000])
        XCTAssertEqual(lines.map(\.text), ["hello", "world"])
    }

    func testFastPathHitCarriesTypedSlots() throws {
        let hit = try XCTUnwrap(nluFastPathMatch(transcript: "repeat off"))
        XCTAssertEqual(hit.intent, "SET_REPEAT")
        XCTAssertEqual(hit.slots.repeatMode, .off)
        XCTAssertNil(nluFastPathMatch(transcript: "play some norwegian jazz"))
    }

    func testRejectionAcceptsAConfidentHead() throws {
        let catalog = nluIntentCatalog()
        XCTAssertEqual(catalog.surfaceNames.count, 22)
        var logits = [Double](repeating: -8.0, count: catalog.surfaceNames.count)
        logits[catalog.surfaceNames.firstIndex(of: "PAUSE")!] = 8.0
        let outcome = try nluRejectionEvaluate(intentLogits: logits, inDomainLogit: 6.0, policy: NluRejectionPolicy())
        XCTAssertEqual(outcome, .accept(intent: "PAUSE"))
    }

    func testManifestRoundTripsThroughTheCoreParser() throws {
        let manifest = try parseOtaDiscoverManifest(json: """
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
        """)
        let latest = try XCTUnwrap(manifest.channels["stable"]).latest
        let composite = try XCTUnwrap(parseOtaCompositeVersion(raw: latest))
        XCTAssertEqual(composite.daemon, "1.2.3")
        XCTAssertEqual(otaCompositeVersionString(version: composite), latest)
        let urls = otaArtifactUrls(
            rootUrl: "https://ota.bridgething.com/", channel: "stable",
            daemonVersion: composite.daemon, imageVersion: composite.image, imageVariant: "prod"
        )
        XCTAssertEqual(urls.daemonBinary, "https://ota.bridgething.com/daemon/stable/1.2.3/bridgething")
    }

    func testRunStoreReducesAPlanThroughToADismissableSuccess() {
        let store = OtaRunStore()
        let steps = [OtaPlanStep(id: 0, kind: .download, label: "update.swu", bytes: 1000)]
        _ = store.ingest(event: .planned(
            deviceId: "dev-1", kind: .image, release: "1+image.2", daemonVersion: "1", imageVersion: "2",
            channel: "stable", rootUrl: "https://ota.bridgething.com", steps: steps
        ))
        _ = store.ingest(event: .progress(
            deviceId: "dev-1", kind: .image, stepId: 0,
            snapshot: .downloading(asset: "update.swu", received: 500, total: 1000, ratePerSec: nil)
        ))
        let run = store.runs()[0]
        XCTAssertEqual(run.lane, .firmware)
        let progress = otaRunProgress(run: run, nowMs: run.phaseStartedAtMs)
        XCTAssertTrue((1 ... 99).contains(progress.percent), "mid-download percent, got \(progress.percent)")
        XCTAssertNil(store.dismiss(deviceId: "dev-1", lane: .firmware), "an unfinished run must refuse dismissal")

        let changes = store.ingest(event: .updated(deviceId: "dev-1", kind: .image, version: "1+image.2"))
        XCTAssertTrue(changes.contains { change in
            if case let .run(run) = change { return run.outcome == .succeeded }
            return false
        })
        XCTAssertNotNil(store.dismiss(deviceId: "dev-1", lane: .firmware))
    }
}
