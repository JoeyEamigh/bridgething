package com.bridgething.companion.shell

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.os.Bundle
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.bridgething_companion.AudioBackend
import uniffi.bridgething_companion.EarconSink
import uniffi.bridgething_companion.SpeakSink

public class AndroidAudioBackend(
    context: Context,
) : AudioBackend {
    private val appContext = context.applicationContext
    private val ready = CompletableDeferred<Boolean>()
    private val callbacks = ConcurrentHashMap<String, SpeakSink>()
    private val watchdogs = ConcurrentHashMap<String, Job>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main + CoroutineName("bridgething-audio"))

    private val speechAttributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_MEDIA)
        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
        .build()

    private val tts = TextToSpeech(appContext) { status ->
        ready.complete(status == TextToSpeech.SUCCESS)
    }.apply {
        setAudioAttributes(speechAttributes)
        setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {
                utteranceId?.let { callbacks[it]?.onStart() }
            }

            override fun onDone(utteranceId: String?) = finish(utteranceId, completed = true)

            @Deprecated("required abstract override; the int-code variant supersedes on newer API levels")
            override fun onError(utteranceId: String?) = finish(utteranceId, completed = false)

            override fun onError(utteranceId: String?, errorCode: Int) = finish(utteranceId, completed = false)

            override fun onStop(utteranceId: String?, interrupted: Boolean) = finish(utteranceId, completed = false)
        })
    }

    private fun finish(utteranceId: String?, completed: Boolean) {
        val id = utteranceId ?: return
        val sink = callbacks.remove(id)
        watchdogs.remove(id)?.cancel()
        sink?.use { it.onFinished(completed) }
    }

    override fun speak(id: String, text: String, voice: String?, sink: SpeakSink) {
        scope.launch {
            if (!ready.await()) {
                sink.use { it.onFinished(false) }
                return@launch
            }
            applyVoice(voice)
            callbacks[id] = sink
            val result = tts.speak(text, TextToSpeech.QUEUE_FLUSH, Bundle(), id)
            if (result != TextToSpeech.SUCCESS) {
                finish(id, completed = false)
                return@launch
            }
            watchdogs[id] = scope.launch {
                delay(speakDeadlineMs(text))
                finish(id, completed = false)
            }
        }
    }

    private fun speakDeadlineMs(text: String): Long =
        maxOf(MIN_SPEAK_DEADLINE_MS, text.length * MS_PER_CHAR + SPEAK_DEADLINE_SLACK_MS)

    private fun applyVoice(voice: String?) {
        if (voice == null) return
        val match = tts.voices?.firstOrNull { it.name == voice }
        if (match != null) {
            tts.voice = match
            return
        }
        runCatching { tts.language = Locale.forLanguageTag(voice) }
    }

    override fun cancel(id: String) {
        if (!callbacks.containsKey(id)) return
        runCatching { tts.stop() }
    }

    override fun cancelAll() {
        runCatching { tts.stop() }
    }

    override fun playEarcon(name: String, sink: EarconSink) {
        val resId = appContext.resources.getIdentifier(name, "raw", appContext.packageName)
        if (resId == 0) {
            sink.use { it.onFinished(false) }
            return
        }
        val player = MediaPlayer.create(appContext, resId)
        if (player == null) {
            sink.use { it.onFinished(false) }
            return
        }
        player.setOnCompletionListener {
            it.release()
            sink.use { held -> held.onFinished(true) }
        }
        player.start()
    }

    private companion object {
        const val MIN_SPEAK_DEADLINE_MS = 15_000L
        const val MS_PER_CHAR = 120L
        const val SPEAK_DEADLINE_SLACK_MS = 10_000L
    }
}
