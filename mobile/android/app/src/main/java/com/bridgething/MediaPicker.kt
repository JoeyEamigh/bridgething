package com.bridgething

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.ext.SdkExtensions
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import com.bridgething.companion.shell.PickStaging
import java.io.File
import java.io.IOException
import uniffi.bridgething_companion.MediaPickKind

public class PickedFile(public val file: File, public val mime: String?)

public object MediaPicker {
    public fun sweep(context: Context) {
        staging(context).sweep()
    }

    public suspend fun <T> pick(context: Context, kind: MediaPickKind, use: suspend (PickedFile) -> T): T? {
        val result = BridgethingActivityRegistry.awaitResult(ActivityRequest.MEDIA_PICK) { activity, requestCode ->
            try {
                activity.startActivityForResult(intentFor(kind), requestCode)
            } catch (_: ActivityNotFoundException) {
                error("unsupported: this phone has no picker for that kind of file")
            }
        }
        val uri = result.data?.data?.takeIf { result.resultCode == Activity.RESULT_OK } ?: return null
        val resolver = context.applicationContext.contentResolver
        val mime = resolver.getType(uri)
        val extension = mime?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) } ?: "bin"
        val open = { resolver.openInputStream(uri) ?: throw IOException("the picked file could not be opened") }
        return staging(context).staged(extension, open) { file -> use(PickedFile(file, mime)) }
    }

    private fun staging(context: Context): PickStaging = PickStaging(context.applicationContext.cacheDir)

    private fun intentFor(kind: MediaPickKind): Intent {
        val type = when (kind) {
            MediaPickKind.IMAGE -> "image/*"
            MediaPickKind.VIDEO -> "video/*"
            MediaPickKind.ANY -> "*/*"
        }
        if (kind != MediaPickKind.ANY && photoPickerAvailable()) {
            return Intent(MediaStore.ACTION_PICK_IMAGES).setType(type)
        }
        return Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType(type)
    }

    private fun photoPickerAvailable(): Boolean = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU -> true
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.R -> SdkExtensions.getExtensionVersion(Build.VERSION_CODES.R) >= 2
        else -> false
    }
}
