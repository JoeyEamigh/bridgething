package com.bridgething

import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothDevice
import android.companion.AssociationRequest
import android.companion.BluetoothDeviceFilter
import android.companion.CompanionDeviceManager
import android.content.Context
import android.content.IntentSender
import com.margelo.nitro.bridgething.session.BridgethingBtBondState
import com.margelo.nitro.bridgething.session.BridgethingBtDevice
import java.util.regex.Pattern

public object CompanionDevicePicker {
    private val carThingNameRegex: Pattern =
        Pattern.compile("(Car Thing|bridgething)", Pattern.CASE_INSENSITIVE)

    @SuppressLint("MissingPermission")
    public suspend fun pick(): BridgethingBtDevice? {
        val result = BridgethingActivityRegistry.awaitResult(ActivityRequest.COMPANION_DEVICE) { activity, requestCode ->
            val manager = activity.getSystemService(CompanionDeviceManager::class.java)
                ?: error("CompanionDeviceManager unavailable on this device")
            manager.associate(associationRequest(), object : CompanionDeviceManager.Callback() {
                override fun onDeviceFound(chooserLauncher: IntentSender) {
                    try {
                        activity.startIntentSenderForResult(chooserLauncher, requestCode, null, 0, 0, 0)
                    } catch (e: IntentSender.SendIntentException) {
                        BridgethingActivityRegistry.deliverResult(requestCode, Activity.RESULT_CANCELED, null)
                        android.util.Log.w(TAG, "CDM intent sender failed: ${e.message}")
                    }
                }

                override fun onFailure(error: CharSequence?) {
                    BridgethingActivityRegistry.deliverResult(requestCode, Activity.RESULT_CANCELED, null)
                    if (!error.isNullOrEmpty()) android.util.Log.i(TAG, "CDM picker failure: $error")
                }
            }, null)
        }
        val data = result.data?.takeIf { result.resultCode == Activity.RESULT_OK } ?: return null
        val device: BluetoothDevice? =
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
                data.getParcelableExtra(CompanionDeviceManager.EXTRA_DEVICE, BluetoothDevice::class.java)
            } else {
                @Suppress("DEPRECATION") data.getParcelableExtra(CompanionDeviceManager.EXTRA_DEVICE)
            }
        if (device == null) return null
        if (device.bondState != BluetoothDevice.BOND_BONDED) {
            BondWatcher.beginPairing(device.address)
            runCatching { device.createBond() }
        }
        return toWireDevice(device)
    }

    private fun associationRequest(): AssociationRequest {
        val builder = AssociationRequest.Builder()
            .addDeviceFilter(
                BluetoothDeviceFilter.Builder()
                    .setNamePattern(carThingNameRegex)
                    .build()
            )
            .setSingleDevice(false)
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            builder.setDeviceProfile(AssociationRequest.DEVICE_PROFILE_WATCH)
        }
        return builder.build()
    }

    public fun associations(context: Context): Set<String> {
        val manager = context.applicationContext
            .getSystemService(Context.COMPANION_DEVICE_SERVICE) as? CompanionDeviceManager
            ?: return emptySet()
        return try {
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
                manager.myAssociations.mapNotNullTo(mutableSetOf()) { it.deviceMacAddress?.toString() }
            } else {
                @Suppress("DEPRECATION") manager.associations.toSet()
            }
        } catch (_: Throwable) { emptySet() }
    }

    public fun startObservingPresence(context: Context) {
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.S) return
        val manager = context.applicationContext
            .getSystemService(Context.COMPANION_DEVICE_SERVICE) as? CompanionDeviceManager ?: return
        for (mac in associations(context)) {
            runCatching {
                @Suppress("DEPRECATION")
                manager.startObservingDevicePresence(mac)
            }
        }
    }

    public fun forget(context: Context, mac: String) {
        val manager = context.applicationContext
            .getSystemService(Context.COMPANION_DEVICE_SERVICE) as? CompanionDeviceManager ?: return
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            val matches = runCatching { manager.myAssociations }.getOrDefault(emptyList())
                .filter { it.deviceMacAddress?.toString().equals(mac, ignoreCase = true) }
            for (assoc in matches) {
                assoc.deviceMacAddress?.toString()?.let { stopObserving(manager, it) }
                runCatching { manager.disassociate(assoc.id) }
            }
        } else {
            stopObserving(manager, mac)
            runCatching { @Suppress("DEPRECATION") manager.disassociate(mac) }
        }
    }

    private fun stopObserving(manager: CompanionDeviceManager, mac: String) {
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.S) return
        runCatching { @Suppress("DEPRECATION") manager.stopObservingDevicePresence(mac) }
    }

    public suspend fun awaitBond(context: Context, mac: String): Boolean {
        val ba = (context.applicationContext.getSystemService(Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager)
            ?.adapter ?: return false
        val device = runCatching { ba.getRemoteDevice(mac.uppercase()) }.getOrNull() ?: return false
        return BondWatcher.awaitBonded(device)
    }

    private fun toWireDevice(device: BluetoothDevice): BridgethingBtDevice {
        val name = try { device.name } catch (_: SecurityException) { null }
        return BridgethingBtDevice(
            address = device.address ?: "",
            name = name,
            bondState = bondStateOf(device),
            isCarThing = true,
        )
    }

    internal fun bondStateOf(device: BluetoothDevice): BridgethingBtBondState =
        when (try { device.bondState } catch (_: SecurityException) { BluetoothDevice.BOND_NONE }) {
            BluetoothDevice.BOND_BONDED -> BridgethingBtBondState.BONDED
            BluetoothDevice.BOND_BONDING -> BridgethingBtBondState.BONDING
            else -> BridgethingBtBondState.NONE
        }

    private const val TAG = "bridgething.cdm"
}
