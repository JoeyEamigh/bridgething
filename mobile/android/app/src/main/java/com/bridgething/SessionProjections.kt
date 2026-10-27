package com.bridgething

import com.margelo.nitro.bridgething.session.BridgethingActiveWebapp
import com.margelo.nitro.bridgething.session.BridgethingAncsAuthStatus
import com.margelo.nitro.bridgething.session.BridgethingAncsAuthStatusEntry
import com.margelo.nitro.bridgething.session.BridgethingAuthKind
import com.margelo.nitro.bridgething.session.BridgethingAuthState
import com.margelo.nitro.bridgething.session.BridgethingCapabilityFlags
import com.margelo.nitro.bridgething.session.BridgethingCompanionDebug
import com.margelo.nitro.bridgething.session.BridgethingDeviceAutoResume
import com.margelo.nitro.bridgething.session.BridgethingDeviceMeta
import com.margelo.nitro.bridgething.session.BridgethingDeviceMetaEntry
import com.margelo.nitro.bridgething.session.BridgethingDeviceWebappsEntry
import com.margelo.nitro.bridgething.session.BridgethingHostInfo
import com.margelo.nitro.bridgething.session.BridgethingLauncherGesture
import com.margelo.nitro.bridgething.session.BridgethingNowPlaying
import com.margelo.nitro.bridgething.session.BridgethingNowPlayingPlayback
import com.margelo.nitro.bridgething.session.BridgethingNowPlayingTrack
import com.margelo.nitro.bridgething.session.BridgethingOtaAvailable
import com.margelo.nitro.bridgething.session.BridgethingOtaChannelInfo
import com.margelo.nitro.bridgething.session.BridgethingOtaKind
import com.margelo.nitro.bridgething.session.BridgethingOtaManifest
import com.margelo.nitro.bridgething.session.BridgethingOtaOutcome
import com.margelo.nitro.bridgething.session.BridgethingOtaPhase
import com.margelo.nitro.bridgething.session.BridgethingOtaPollConfig
import com.margelo.nitro.bridgething.session.BridgethingOtaPollStatus
import com.margelo.nitro.bridgething.session.BridgethingOtaQueue
import com.margelo.nitro.bridgething.session.BridgethingOtaQueuedInstall
import com.margelo.nitro.bridgething.session.BridgethingOtaProgress
import com.margelo.nitro.bridgething.session.BridgethingOtaRelease
import com.margelo.nitro.bridgething.session.BridgethingOtaRun
import com.margelo.nitro.bridgething.session.BridgethingOtaStep
import com.margelo.nitro.bridgething.session.BridgethingOtaStepKind
import com.margelo.nitro.bridgething.session.BridgethingPeerLinkStatus
import com.margelo.nitro.bridgething.session.BridgethingPickKind
import com.margelo.nitro.bridgething.session.BridgethingPickedMedia
import com.margelo.nitro.bridgething.session.BridgethingProviderInfo
import com.margelo.nitro.bridgething.session.BridgethingRepeatMode
import com.margelo.nitro.bridgething.session.BridgethingResumeTarget
import com.margelo.nitro.bridgething.session.BridgethingServiceHealth
import com.margelo.nitro.bridgething.session.BridgethingServiceHealthKind
import com.margelo.nitro.bridgething.session.BridgethingSessionPeer
import com.margelo.nitro.bridgething.session.BridgethingSessionSnapshot
import com.margelo.nitro.bridgething.session.BridgethingSignInMethod
import com.margelo.nitro.bridgething.session.BridgethingVoiceDebug
import com.margelo.nitro.bridgething.session.BridgethingVoiceModelState
import com.margelo.nitro.bridgething.session.BridgethingVoiceModelStatus
import com.margelo.nitro.bridgething.session.BridgethingVoiceTurn
import com.margelo.nitro.bridgething.session.BridgethingVoiceTurnPhase
import com.margelo.nitro.bridgething.session.BridgethingVoiceTurnTrigger
import com.margelo.nitro.bridgething.session.BridgethingWebappBlob
import com.margelo.nitro.bridgething.session.BridgethingWebappInfo
import com.margelo.nitro.bridgething.session.BridgethingWebappRole
import com.margelo.nitro.bridgething.session.BridgethingWebappSlot
import com.margelo.nitro.bridgething.session.BridgethingWebappSlots
import com.margelo.nitro.bridgething.session.BridgethingWebappSource
import java.io.IOException
import kotlin.math.floor
import kotlinx.coroutines.CancellationException
import uniffi.bridgething_companion.ActiveWebapp
import uniffi.bridgething_companion.AncsAuthStatus
import uniffi.bridgething_companion.AuthKind
import uniffi.bridgething_companion.AuthState
import uniffi.bridgething_companion.CapabilityFlags
import uniffi.bridgething_companion.CompanionDebug
import uniffi.bridgething_companion.CompanionException
import uniffi.bridgething_companion.DeviceMeta
import uniffi.bridgething_companion.DeviceWebappsEntry
import uniffi.bridgething_companion.LauncherGesture
import uniffi.bridgething_companion.LogLevel
import uniffi.bridgething_companion.LogOrigin
import uniffi.bridgething_companion.LogStoreLevel
import uniffi.bridgething_companion.MediaCover
import uniffi.bridgething_companion.MediaPickKind
import uniffi.bridgething_companion.NowPlaying
import uniffi.bridgething_companion.OtaAvailable
import uniffi.bridgething_companion.OtaDiscoverManifest
import uniffi.bridgething_companion.OtaKind
import uniffi.bridgething_companion.OtaPollConfig
import uniffi.bridgething_companion.OtaPollStatus
import uniffi.bridgething_companion.OtaQueue
import uniffi.bridgething_companion.OtaRun
import uniffi.bridgething_companion.OtaRunOutcome
import uniffi.bridgething_companion.OtaRunPhase
import uniffi.bridgething_companion.OtaRunProgress
import uniffi.bridgething_companion.OtaStepKind
import uniffi.bridgething_companion.PeerLinkStatus
import uniffi.bridgething_companion.PickedMedia
import uniffi.bridgething_companion.ProviderInfo
import uniffi.bridgething_companion.RepeatMode
import uniffi.bridgething_companion.ResumeTarget
import uniffi.bridgething_companion.ServiceHealthKind
import uniffi.bridgething_companion.SessionPeer
import uniffi.bridgething_companion.SessionSnapshot
import uniffi.bridgething_companion.SignInMethod
import uniffi.bridgething_companion.VoiceModelState
import uniffi.bridgething_companion.VoiceModelStatus
import uniffi.bridgething_companion.VoiceTurn
import uniffi.bridgething_companion.VoiceTurnPhase
import uniffi.bridgething_companion.VoiceTurnTrigger
import uniffi.bridgething_companion.WebappBlob
import uniffi.bridgething_companion.WebappInfo
import uniffi.bridgething_companion.WebappRole
import uniffi.bridgething_companion.WebappSlot
import uniffi.bridgething_companion.WebappSlots
import uniffi.bridgething_companion.WebappSource
import uniffi.bridgething_companion.parseOtaCompositeVersion

internal fun toRnSnapshot(snap: SessionSnapshot): BridgethingSessionSnapshot = BridgethingSessionSnapshot(
    hostInfo = BridgethingHostInfo(
        appName = snap.hostInfo.appName,
        appVersion = snap.hostInfo.appVersion,
        osName = snap.hostInfo.osName,
        osVersion = snap.hostInfo.osVersion,
        hostIdentifier = snap.hostInfo.hostIdentifier,
        libVersion = snap.hostInfo.libVersion,
        libbridgethingVersion = snap.hostInfo.libbridgethingVersion,
        adapterVersion = "rfcomm",
    ),
    providers = snap.providers.map(::toRnProviderInfo).toTypedArray(),
    providerPriority = snap.providerPriority.toTypedArray(),
    libraryProvider = snap.libraryProvider,
    peers = snap.peers.map(::toRnPeer).toTypedArray(),
    ancsAuthStatuses = snap.ancsAuthStatuses.map {
        BridgethingAncsAuthStatusEntry(deviceId = it.deviceId, status = toRnAncsAuthStatus(it.status))
    }.toTypedArray(),
    nowPlaying = snap.nowPlaying?.let(::toRnNowPlaying),
    deviceMeta = snap.deviceMeta.map {
        BridgethingDeviceMetaEntry(deviceId = it.deviceId, meta = toRnDeviceMeta(it.meta))
    }.toTypedArray(),
    capabilityFlags = toRnCapabilityFlags(snap.capabilityFlags),
    voiceModel = toRnVoiceModelState(snap.voiceModel),
    otaPollConfig = snap.otaPollConfig?.let(::toRnOtaPollConfig),
    webapps = snap.webapps.map(::toRnWebappsEntry).toTypedArray(),
    otaRuns = snap.otaRuns.map(::toRnOtaRun).toTypedArray(),
    otaAvailable = snap.otaAvailable.map(::toRnOtaAvailable).toTypedArray(),
    otaPoll = toRnOtaPollStatus(snap.otaPoll),
    otaQueues = snap.otaQueues.map(::toRnOtaQueue).toTypedArray(),
)

internal fun toRnProviderInfo(info: ProviderInfo): BridgethingProviderInfo = BridgethingProviderInfo(
    id = info.id,
    displayName = info.displayName,
    available = info.available,
    connected = info.connected,
    signIn = when (info.signIn) {
        SignInMethod.HANDSHAKE -> BridgethingSignInMethod.HANDSHAKE
        SignInMethod.SERVER_LOGIN -> BridgethingSignInMethod.SERVERLOGIN
    },
    authState = toRnAuthState(info.authState),
    serviceHealth = BridgethingServiceHealth(
        kind = when (info.serviceHealth.kind) {
            ServiceHealthKind.OK -> BridgethingServiceHealthKind.OK
            ServiceHealthKind.RATE_LIMITED -> BridgethingServiceHealthKind.RATELIMITED
            ServiceHealthKind.UNREACHABLE -> BridgethingServiceHealthKind.UNREACHABLE
        },
        retryAfterSeconds = info.serviceHealth.retryAfterSeconds?.toDouble(),
    ),
)

internal fun toRnAuthState(state: AuthState): BridgethingAuthState = BridgethingAuthState(
    kind = when (state.kind) {
        AuthKind.IDLE -> BridgethingAuthKind.IDLE
        AuthKind.PENDING -> BridgethingAuthKind.PENDING
        AuthKind.AUTHENTICATED -> BridgethingAuthKind.AUTHENTICATED
        AuthKind.FAILED -> BridgethingAuthKind.FAILED
    },
    userCode = state.userCode,
    verificationUrl = state.verificationUrl,
    verificationUrlComplete = state.verificationUrlComplete,
    message = state.message,
)

internal fun toRnPeer(peer: SessionPeer): BridgethingSessionPeer = BridgethingSessionPeer(
    id = peer.id,
    name = peer.name,
    status = when (peer.status) {
        PeerLinkStatus.CONNECTED -> BridgethingPeerLinkStatus.CONNECTED
        PeerLinkStatus.LINK_FAILED -> BridgethingPeerLinkStatus.LINKFAILED
    },
    linkError = peer.linkError,
)

internal fun toRnNowPlaying(np: NowPlaying): BridgethingNowPlaying = BridgethingNowPlaying(
    track = np.track?.let { t ->
        BridgethingNowPlayingTrack(
            id = t.id,
            title = t.title,
            artist = t.artist,
            album = t.album,
            artworkUrl = t.artworkUrl,
            durationMs = t.durationMs?.toDouble(),
        )
    },
    playback = BridgethingNowPlayingPlayback(
        playing = np.playback.playing,
        positionMs = np.playback.positionMs.toDouble(),
        shuffle = np.playback.shuffle,
        repeatMode = when (np.playback.repeatMode) {
            RepeatMode.OFF -> BridgethingRepeatMode.OFF
            RepeatMode.ONE -> BridgethingRepeatMode.ONE
            RepeatMode.ALL -> BridgethingRepeatMode.ALL
        },
    ),
    appName = np.appName,
)

internal fun toRnAncsAuthStatus(state: AncsAuthStatus): BridgethingAncsAuthStatus = when (state) {
    AncsAuthStatus.UNKNOWN -> BridgethingAncsAuthStatus.UNKNOWN
    AncsAuthStatus.PROBING -> BridgethingAncsAuthStatus.PROBING
    AncsAuthStatus.AUTHORIZED -> BridgethingAncsAuthStatus.AUTHORIZED
    AncsAuthStatus.UNAUTHORIZED -> BridgethingAncsAuthStatus.UNAUTHORIZED
}

internal fun toRnDeviceMeta(meta: DeviceMeta): BridgethingDeviceMeta = BridgethingDeviceMeta(
    daemonVersion = meta.daemonVersion,
    libbridgethingVersion = meta.libbridgethingVersion,
    imageVersion = meta.imageVersion,
    appName = meta.appName,
    osName = meta.osName,
    osVersion = meta.osVersion,
    channel = meta.channel,
    serialNumber = meta.serialNumber,
    nickname = meta.nickname,
    launcherGesture = toRnLauncherGesture(meta.launcherGesture),
    companionLaunch = meta.companionLaunch,
)

internal fun toRnLauncherGesture(gesture: LauncherGesture): BridgethingLauncherGesture = when (gesture) {
    LauncherGesture.LONG_PRESS -> BridgethingLauncherGesture.LONGPRESS
    LauncherGesture.FIVE_PRESS -> BridgethingLauncherGesture.FIVEPRESS
    LauncherGesture.DOUBLE_PRESS -> BridgethingLauncherGesture.DOUBLEPRESS
}

internal fun toLauncherGesture(gesture: BridgethingLauncherGesture): LauncherGesture = when (gesture) {
    BridgethingLauncherGesture.LONGPRESS -> LauncherGesture.LONG_PRESS
    BridgethingLauncherGesture.FIVEPRESS -> LauncherGesture.FIVE_PRESS
    BridgethingLauncherGesture.DOUBLEPRESS -> LauncherGesture.DOUBLE_PRESS
}

internal fun toRnCapabilityFlags(flags: CapabilityFlags): BridgethingCapabilityFlags = BridgethingCapabilityFlags(
    geo = flags.geo,
    notifications = flags.notifications,
    netFetch = flags.netFetch,
    netWs = flags.netWs,
    audioTts = flags.audioTts,
    voiceModel = flags.voiceModel,
)

internal fun toCoreCapabilityFlags(flags: BridgethingCapabilityFlags): CapabilityFlags = CapabilityFlags(
    geo = flags.geo,
    notifications = flags.notifications,
    netFetch = flags.netFetch,
    netWs = flags.netWs,
    audioTts = flags.audioTts,
    voiceModel = flags.voiceModel,
)

internal fun toRnVoiceTurn(turn: VoiceTurn): BridgethingVoiceTurn = BridgethingVoiceTurn(
    deviceId = turn.deviceId,
    streamId = turn.streamId,
    trigger = when (turn.trigger) {
        VoiceTurnTrigger.PUSH_TO_TALK -> BridgethingVoiceTurnTrigger.PUSHTOTALK
        VoiceTurnTrigger.ASSISTANT -> BridgethingVoiceTurnTrigger.ASSISTANT
        VoiceTurnTrigger.WAKE_WORD -> BridgethingVoiceTurnTrigger.WAKEWORD
    },
    phase = when (turn.phase) {
        VoiceTurnPhase.LISTENING -> BridgethingVoiceTurnPhase.LISTENING
        VoiceTurnPhase.RESOLVED -> BridgethingVoiceTurnPhase.RESOLVED
        VoiceTurnPhase.CANCELLED -> BridgethingVoiceTurnPhase.CANCELLED
    },
    transcript = turn.transcript,
    intent = turn.intent,
)

internal fun toRnVoiceModelState(state: VoiceModelState): BridgethingVoiceModelState = BridgethingVoiceModelState(
    status = when (state.status) {
        VoiceModelStatus.ABSENT -> BridgethingVoiceModelStatus.ABSENT
        VoiceModelStatus.DOWNLOADING -> BridgethingVoiceModelStatus.DOWNLOADING
        VoiceModelStatus.READY -> BridgethingVoiceModelStatus.READY
        VoiceModelStatus.FAILED -> BridgethingVoiceModelStatus.FAILED
    },
    receivedBytes = state.receivedBytes.toDouble(),
    totalBytes = state.totalBytes.toDouble(),
    version = state.version,
    error = state.error,
)

internal fun toRnWebappsEntry(entry: DeviceWebappsEntry): BridgethingDeviceWebappsEntry =
    BridgethingDeviceWebappsEntry(
        deviceId = entry.deviceId,
        webapps = entry.webapps.map(::toRnWebappInfo).toTypedArray(),
        active = entry.active?.let(::toRnActiveWebapp),
        listed = entry.listed,
    )

internal fun toRnActiveWebapp(active: ActiveWebapp): BridgethingActiveWebapp =
    BridgethingActiveWebapp(id = active.id, name = active.name)

internal fun toRnWebappInfo(info: WebappInfo): BridgethingWebappInfo = BridgethingWebappInfo(
    id = info.id,
    name = info.name,
    source = when (info.source) {
        WebappSource.BUILTIN -> BridgethingWebappSource.BUILTIN
        WebappSource.INSTALLED -> BridgethingWebappSource.INSTALLED
    },
    role = when (info.role) {
        WebappRole.LAUNCHER -> BridgethingWebappRole.LAUNCHER
        WebappRole.STANDARD -> BridgethingWebappRole.STANDARD
    },
    version = info.version,
    provenance = info.provenance,
    description = info.description,
    iconHash = info.iconHash,
    settingsHash = info.settingsHash,
    overlayHash = info.overlayHash,
    configJson = info.config,
    permissions = info.permissions.toTypedArray(),
)

internal fun toCorePickKind(kind: BridgethingPickKind): MediaPickKind = when (kind) {
    BridgethingPickKind.IMAGE -> MediaPickKind.IMAGE
    BridgethingPickKind.VIDEO -> MediaPickKind.VIDEO
    BridgethingPickKind.ANY -> MediaPickKind.ANY
}

internal fun toCoreCover(width: Double?, height: Double?): MediaCover? {
    if (width == null && height == null) return null
    return MediaCover(coverEdge(width), coverEdge(height))
}

private fun coverEdge(value: Double?): UInt {
    if (value == null || value != floor(value) || value < 1.0 || value > UInt.MAX_VALUE.toDouble()) {
        throw PlainFailure("invalid: cover edges must be whole numbers from 1 to ${UInt.MAX_VALUE}")
    }
    return value.toUInt()
}

internal fun toRnPickedMedia(media: PickedMedia): BridgethingPickedMedia = BridgethingPickedMedia(
    handle = media.handle,
    mime = media.mime,
    size = media.size.toDouble(),
    width = media.width?.toDouble(),
    height = media.height?.toDouble(),
    durationMs = media.durationMs?.toDouble(),
    preview = media.preview,
)

internal fun toRnWebappBlob(blob: WebappBlob): BridgethingWebappBlob =
    BridgethingWebappBlob(key = blob.key, sha256 = blob.sha256, mime = blob.mime, size = blob.size.toDouble())

private class PlainFailure(message: String) : Exception(message, null, false, false) {
    override fun toString(): String = message.orEmpty()
}

internal suspend fun <T> plainly(work: suspend () -> T): T = try {
    work()
} catch (error: CancellationException) {
    throw error
} catch (error: CompanionException) {
    throw PlainFailure(error.message.orEmpty())
} catch (error: IllegalStateException) {
    throw PlainFailure(error.message.orEmpty())
} catch (error: IOException) {
    throw PlainFailure("io: ${error.message.orEmpty()}")
}

internal fun toRnWebappSlots(slots: WebappSlots): BridgethingWebappSlots = BridgethingWebappSlots(
    launcher = slots.launcher,
    overlay = slots.overlay,
)

internal fun toCoreWebappSlot(slot: BridgethingWebappSlot): WebappSlot = when (slot) {
    BridgethingWebappSlot.LAUNCHER -> WebappSlot.LAUNCHER
    BridgethingWebappSlot.OVERLAY -> WebappSlot.OVERLAY
}

internal fun toRnOtaManifest(m: OtaDiscoverManifest): BridgethingOtaManifest {
    val channels = m.channels.map { (slug, ch) ->
        val releases = ch.releases.mapNotNull { v ->
            val composite = parseOtaCompositeVersion(v) ?: return@mapNotNull null
            val rel = m.releases[v]
            BridgethingOtaRelease(
                version = v,
                daemonVersion = composite.daemon,
                imageVersion = composite.image,
                yanked = rel?.yanked != null,
                deprecated = rel?.deprecated ?: false,
            )
        }.toTypedArray()
        BridgethingOtaChannelInfo(
            slug = slug,
            name = ch.name,
            stability = ch.stability,
            isDefault = ch.isDefault,
            latest = ch.latest,
            releases = releases,
        )
    }.toTypedArray()
    return BridgethingOtaManifest(updatedAt = m.updatedAt, channels = channels)
}

internal fun toRnOtaRun(run: OtaRun): BridgethingOtaRun = BridgethingOtaRun(
    runId = run.runId,
    deviceId = run.deviceId,
    otaKind = toRnOtaKind(run.kind),
    phase = toRnOtaPhase(run.phase),
    steps = run.steps.map {
        BridgethingOtaStep(it.id.toDouble(), toRnOtaStepKind(it.kind), it.label, it.bytes.toDouble())
    }.toTypedArray(),
    stepId = run.stepId.toDouble(),
    startedAt = run.startedAtMs.toDouble(),
    phaseStartedAt = run.phaseStartedAtMs.toDouble(),
    stageReceived = run.stageReceived?.toDouble(),
    stageTotal = run.stageTotal?.toDouble(),
    ratePerSec = run.ratePerSec,
    writePercent = run.writePercent?.toDouble(),
    outcome = run.outcome?.let(::toRnOtaOutcome),
    error = run.error,
    releaseVersion = run.releaseVersion,
    daemonVersion = run.daemonVersion,
    imageVersion = run.imageVersion,
    resumable = run.resumable,
    webappId = run.webappId,
    webappName = run.webappName,
)

internal fun toRnOtaProgress(progress: OtaRunProgress): BridgethingOtaProgress = BridgethingOtaProgress(
    percent = progress.percent.toDouble(),
    stepIndex = progress.stepIndex.toDouble(),
    stepCount = progress.stepCount.toDouble(),
    stepLabel = progress.stepLabel,
    etaSeconds = progress.etaSeconds?.toDouble(),
)

internal fun toRnOtaPhase(p: OtaRunPhase): BridgethingOtaPhase = when (p) {
    OtaRunPhase.IDLE -> BridgethingOtaPhase.IDLE
    OtaRunPhase.DOWNLOADING -> BridgethingOtaPhase.DOWNLOADING
    OtaRunPhase.STREAMING -> BridgethingOtaPhase.STREAMING
    OtaRunPhase.VERIFYING -> BridgethingOtaPhase.VERIFYING
    OtaRunPhase.WRITING -> BridgethingOtaPhase.WRITING
    OtaRunPhase.CONFIRMING -> BridgethingOtaPhase.CONFIRMING
    OtaRunPhase.REBOOT -> BridgethingOtaPhase.REBOOT
    OtaRunPhase.COMPLETED -> BridgethingOtaPhase.COMPLETED
    OtaRunPhase.FAILED -> BridgethingOtaPhase.FAILED
}

internal fun toRnOtaOutcome(o: OtaRunOutcome): BridgethingOtaOutcome = when (o) {
    OtaRunOutcome.SUCCEEDED -> BridgethingOtaOutcome.SUCCEEDED
    OtaRunOutcome.FAILED -> BridgethingOtaOutcome.FAILED
    OtaRunOutcome.CANCELLED -> BridgethingOtaOutcome.CANCELLED
}

internal fun toRnOtaKind(k: OtaKind): BridgethingOtaKind = when (k) {
    OtaKind.IMAGE -> BridgethingOtaKind.IMAGE
    OtaKind.DAEMON -> BridgethingOtaKind.DAEMON
    OtaKind.BUILTIN_WEBAPP -> BridgethingOtaKind.BUILTINWEBAPP
    OtaKind.INSTALLED_WEBAPP -> BridgethingOtaKind.INSTALLEDWEBAPP
    OtaKind.WAKEWORD_MODEL -> BridgethingOtaKind.WAKEWORDMODEL
}

internal fun toRnOtaStepKind(k: OtaStepKind): BridgethingOtaStepKind = when (k) {
    OtaStepKind.DOWNLOAD -> BridgethingOtaStepKind.DOWNLOAD
    OtaStepKind.STREAM -> BridgethingOtaStepKind.STREAM
    OtaStepKind.APPLY -> BridgethingOtaStepKind.APPLY
    OtaStepKind.REBOOT -> BridgethingOtaStepKind.REBOOT
}

internal fun toRnOtaAvailable(a: OtaAvailable): BridgethingOtaAvailable = BridgethingOtaAvailable(
    deviceId = a.deviceId,
    releaseVersion = a.releaseVersion,
    daemonVersion = a.daemonVersion,
    imageVersion = a.imageVersion,
)

internal fun toRnOtaPollStatus(s: OtaPollStatus): BridgethingOtaPollStatus =
    BridgethingOtaPollStatus(lastPolledAt = s.lastPolledAt, error = s.error)

internal fun toRnOtaQueue(q: OtaQueue): BridgethingOtaQueue = BridgethingOtaQueue(
    deviceId = q.deviceId,
    installs = q.installs.map {
        BridgethingOtaQueuedInstall(webappId = it.webappId, webappName = it.webappName, version = it.version)
    }.toTypedArray(),
)

internal fun toRnOtaPollConfig(config: OtaPollConfig): BridgethingOtaPollConfig = BridgethingOtaPollConfig(
    intervalSeconds = config.intervalSeconds.toDouble(),
    autoPush = config.autoPush,
    rootUrl = config.rootUrl,
)

internal fun toCoreOtaPollConfig(config: BridgethingOtaPollConfig): OtaPollConfig = OtaPollConfig(
    intervalSeconds = config.intervalSeconds.toLong().coerceAtLeast(60L).toULong(),
    autoPush = config.autoPush,
    rootUrl = config.rootUrl,
)

internal fun toCoreResumeTarget(target: BridgethingResumeTarget): ResumeTarget = when (target) {
    BridgethingResumeTarget.PHONEONLY -> ResumeTarget.PHONE_ONLY
    BridgethingResumeTarget.ANYSPEAKER -> ResumeTarget.ANY_SPEAKER
}

internal fun toRnResumeTarget(target: ResumeTarget): BridgethingResumeTarget = when (target) {
    ResumeTarget.PHONE_ONLY -> BridgethingResumeTarget.PHONEONLY
    ResumeTarget.ANY_SPEAKER -> BridgethingResumeTarget.ANYSPEAKER
}

internal const val DAEMON_LABEL: String = "daemon"

internal fun toOriginName(origin: LogOrigin): String = when (origin) {
    LogOrigin.DEVICE -> "device"
    LogOrigin.HOST -> "local"
}

internal fun toLevelName(level: LogLevel): String = when (level) {
    LogLevel.TRACE, LogLevel.DEBUG -> "debug"
    LogLevel.INFO -> "info"
    LogLevel.WARN -> "warn"
    LogLevel.ERROR -> "error"
}

internal fun toLevelName(level: LogStoreLevel): String = when (level) {
    LogStoreLevel.TRACE, LogStoreLevel.DEBUG -> "debug"
    LogStoreLevel.INFO, LogStoreLevel.NOTICE -> "info"
    LogStoreLevel.WARN -> "warn"
    LogStoreLevel.ERROR, LogStoreLevel.FATAL -> "error"
}

internal fun toRnCompanionDebug(debug: CompanionDebug): BridgethingCompanionDebug = BridgethingCompanionDebug(
    authorityPlaybackHeld = debug.authorityPlaybackHeld,
    authorityMetadataHeld = debug.authorityMetadataHeld,
    authorityVolumeHeld = debug.authorityVolumeHeld,
    authorityAppBundle = debug.authorityAppBundle,
    arbitratedSource = debug.arbitratedSource,
    librarySource = debug.librarySource,
    lastPlayedFrom = debug.lastPlayedFrom,
    attachedProviders = debug.attachedProviders.toTypedArray(),
    attachedSchemes = debug.attachedSchemes.toTypedArray(),
    linkedDevices = debug.linkedDevices.toTypedArray(),
    autoResume = debug.autoResume
        .map { BridgethingDeviceAutoResume(deviceId = it.deviceId, enabled = it.enabled) }
        .toTypedArray(),
    voice = BridgethingVoiceDebug(
        hasModel = debug.voice.hasModel,
        armedBundle = debug.voice.armedBundle,
        transferAllowed = debug.voice.transferAllowed,
        nluBundleDir = debug.voice.paths.nluBundleDir,
        asrWeights = debug.voice.paths.asrWeights,
    ),
)

internal fun toStoreLevel(level: LogLevel): LogStoreLevel = when (level) {
    LogLevel.TRACE -> LogStoreLevel.TRACE
    LogLevel.DEBUG -> LogStoreLevel.DEBUG
    LogLevel.INFO -> LogStoreLevel.INFO
    LogLevel.WARN -> LogStoreLevel.WARN
    LogLevel.ERROR -> LogStoreLevel.ERROR
}
