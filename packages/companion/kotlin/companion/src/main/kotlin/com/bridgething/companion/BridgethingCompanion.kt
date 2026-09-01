package com.bridgething.companion

import android.app.PendingIntent
import android.bluetooth.BluetoothManager
import android.content.ComponentName
import android.content.Context
import com.bridgething.companion.shell.AndroidAudioBackend
import com.bridgething.companion.shell.AndroidConnectivityMonitor
import com.bridgething.companion.shell.AndroidGeoProvider
import com.bridgething.companion.shell.AndroidHostEnvironment
import com.bridgething.companion.shell.AndroidImageScaler
import com.bridgething.companion.shell.AndroidMediaSessionBackend
import com.bridgething.companion.shell.AndroidNotificationBackend
import com.bridgething.companion.shell.AndroidPhoneBackend
import com.bridgething.companion.shell.AndroidVolumeBackend
import com.bridgething.companion.shell.BtLinkTransport
import com.bridgething.companion.shell.EncryptedPrefsSecretStore
import com.bridgething.companion.shell.IntentDeviceWaker
import com.bridgething.companion.shell.KtorHttpTransport
import com.bridgething.companion.shell.KtorWsTransport
import com.bridgething.companion.shell.LitertArtifactValidator
import com.bridgething.companion.shell.LitertNluRunner
import com.bridgething.companion.shell.LogcatSink
import com.bridgething.companion.shell.SystemTimeWatcher
import com.bridgething.companion.shell.UnmeteredTransferPolicy
import com.bridgething.companion.shell.WhisperSpeechBackend
import uniffi.bridgething_companion.CapabilityFlags
import uniffi.bridgething_companion.CompanionBackends
import uniffi.bridgething_companion.CompanionConfig
import uniffi.bridgething_companion.CompanionSession
import uniffi.bridgething_companion.HostInfo
import uniffi.bridgething_companion.ModelPlatform
import uniffi.bridgething_companion.SessionEvent
import uniffi.bridgething_companion.SessionEventSink
import uniffi.bridgething_companion.SessionSnapshot
import uniffi.bridgething_companion.SpotifyProviderConfig

public class BridgethingCompanion(
    context: Context,
    host: HostInfo,
    capabilities: CapabilityFlags,
    resolveNotificationAction: (id: String, positive: Boolean) -> PendingIntent?,
    notificationListener: ComponentName,
    spotify: SpotifyProviderConfig?,
    events: (SessionEvent) -> Unit,
) {
    public val transport: BtLinkTransport = BtLinkTransport(
        bluetooth = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter,
    )

    public val notifications: AndroidNotificationBackend = AndroidNotificationBackend(resolveNotificationAction)

    public val mediaSessions: AndroidMediaSessionBackend =
        AndroidMediaSessionBackend(context.applicationContext, notificationListener)

    private val http: KtorHttpTransport = KtorHttpTransport()
    private val ws: KtorWsTransport = KtorWsTransport()

    public val session: CompanionSession

    init {
        val appContext = context.applicationContext
        val secrets = EncryptedPrefsSecretStore(appContext)
        session = CompanionSession.create(
            config = CompanionConfig(
                host = host,
                capabilities = capabilities,
                stateDir = appContext.filesDir.absolutePath,
                cacheDir = appContext.cacheDir.absolutePath,
                modelPlatform = ModelPlatform.ANDROID,
                spotify = spotify,
            ),
            backends = CompanionBackends(
                link = transport,
                host = AndroidHostEnvironment(),
                http = http,
                ws = ws,
                secrets = secrets,
                log = LogcatSink(),
                audio = AndroidAudioBackend(appContext),
                volume = AndroidVolumeBackend(appContext),
                geo = AndroidGeoProvider(appContext),
                notifications = notifications,
                phone = AndroidPhoneBackend(appContext),
                mediaSessions = mediaSessions,
                speech = WhisperSpeechBackend { session.voiceModelPaths().asrWeights },
                nlu = LitertNluRunner { session.voiceModelPaths().nluBundleDir },
                appleMusic = null,
                image = AndroidImageScaler(),
                modelValidator = LitertArtifactValidator(),
                transferPolicy = UnmeteredTransferPolicy(appContext),
                connectivity = AndroidConnectivityMonitor(appContext),
                deviceWaker = IntentDeviceWaker(appContext),
            ),
            events = object : SessionEventSink {
                override fun onEvent(event: SessionEvent) = events(event)
            },
        )
    }

    private val timeWatcher: SystemTimeWatcher = SystemTimeWatcher(context) { session.timeChanged() }

    public suspend fun start() {
        session.start()
        timeWatcher.start()
    }

    public suspend fun stop() {
        timeWatcher.stop()
        session.stop()
        http.close()
        ws.close()
    }

    public suspend fun snapshot(): SessionSnapshot = session.snapshot()
}
