package com.bridgething

import android.app.Application
import com.bridgething.companion.CompanionLogs
import com.bridgething.companion.CrashCapture
import com.bridgething.companion.LogcatCapture
import com.facebook.react.PackageList
import com.facebook.react.ReactApplication
import com.facebook.react.ReactHost
import com.facebook.react.ReactNativeApplicationEntryPoint.loadReactNative
import com.facebook.react.defaults.DefaultReactHost.getDefaultReactHost

class MainApplication : Application(), ReactApplication {

  override val reactHost: ReactHost by lazy {
    getDefaultReactHost(
      context = applicationContext,
      packageList =
        PackageList(this).packages.apply {
          // Packages that cannot be autolinked yet can be added manually here, for example:
          // add(MyReactNativePackage())
        },
    )
  }

  override fun onCreate() {
    super.onCreate()
    CompanionLogs.install(this)
    CrashCapture.install()
    LogcatCapture.start()
    BridgethingActivityRegistry.installCallbacks(this)
    BridgethingApp.installBridgething(this)
    loadReactNative(this)
  }
}
