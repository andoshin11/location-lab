package dev.locationlab.helper

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.location.Criteria
import android.location.Location
import android.location.LocationManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import java.io.FileDescriptor
import java.io.PrintWriter
import java.util.Locale

/**
 * Android 公式の mock location（テストプロバイダ）機構で位置を差し替える Foreground Service。
 *
 * Mac からは adb 経由で `am start-foreground-service -a <ACTION> --es <key> <value>` で操作し、
 * 状態は `dumpsys activity service` で取得する（[dump] 参照）。
 * 位置履歴は保存しない。ログにも座標は出さない。
 */
class MockLocationService : Service() {

    enum class State { IDLE, HOLDING, WALKING, PAUSED, ARRIVED, ERROR }

    data class Status(
        val state: State = State.IDLE,
        val position: LatLng? = null,
        val bearing: Double = 0.0,
        val speedMps: Double = 0.0,
        val remainingMeters: Double = 0.0,
        val remainingSeconds: Double = 0.0,
        val providers: List<String> = emptyList(),
        val error: String? = null,
    )

    private val handler = Handler(Looper.getMainLooper())
    private lateinit var locationManager: LocationManager

    private var activeProviders: List<String> = emptyList()
    private var position: LatLng? = null
    private var bearing: Double = 0.0
    private var walk: WalkSimulator? = null
    private var lastTickAt: Long = 0L

    private val tick = object : Runnable {
        override fun run() {
            onTick()
            handler.postDelayed(this, TICK_INTERVAL_MS)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        locationManager = getSystemService(Context.LOCATION_SERVICE) as LocationManager
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // startForegroundService() から 5 秒以内に必ず startForeground() する
        enterForeground()
        try {
            handle(intent)
        } catch (e: IllegalArgumentException) {
            fail("invalid argument: ${e.message}")
        } catch (e: SecurityException) {
            fail(MOCK_APP_NOT_SELECTED)
        }
        return START_NOT_STICKY
    }

    private fun handle(intent: Intent?) {
        when (intent?.action) {
            ACTION_SET -> {
                val point = LatLng.parse(intent.requireString(EXTRA_POINT))
                if (!ensureMocking()) return
                walk = null
                position = point
                publishNow(State.HOLDING)
            }

            ACTION_WALK -> {
                val waypoints = LatLng.parseList(intent.requireString(EXTRA_TO))
                require(waypoints.isNotEmpty()) { "destination is empty" }
                val speed = intent.getStringExtra(EXTRA_SPEED)?.let {
                    it.toDoubleOrNull() ?: throw IllegalArgumentException("invalid speed: $it")
                } ?: WalkSimulator.DEFAULT_WALK_SPEED_MPS
                WalkSimulator.validateSpeed(speed)
                val from = intent.getStringExtra(EXTRA_FROM)?.let(LatLng::parse)
                    ?: position
                    ?: lastKnownRealLocation()
                    ?: return fail(START_UNKNOWN)
                if (!ensureMocking()) return
                walk = WalkSimulator(Route(listOf(from) + waypoints), speed)
                position = from
                lastTickAt = SystemClock.elapsedRealtime()
                publishNow(State.WALKING)
            }

            ACTION_PAUSE -> walk?.let {
                it.paused = true
                publishNow(State.PAUSED)
            }

            ACTION_RESUME -> walk?.let {
                it.paused = false
                lastTickAt = SystemClock.elapsedRealtime()
                publishNow(if (it.arrived) State.ARRIVED else State.WALKING)
            }

            ACTION_SPEED -> {
                val speed = intent.requireString(EXTRA_SPEED).toDoubleOrNull()
                    ?: throw IllegalArgumentException("invalid speed")
                walk?.speedMetersPerSecond = speed
                updateStatus(currentStatus.state)
            }

            ACTION_STOP -> stopMocking()

            // 状態確認のためだけに起動された場合など
            else -> if (activeProviders.isEmpty() && currentStatus.state != State.ERROR) stopMocking()
        }
    }

    private fun Intent.requireString(key: String): String =
        getStringExtra(key) ?: throw IllegalArgumentException("missing extra: $key")

    // ---- mock provider の登録・解除 ----

    /** テストプロバイダを登録する。既に登録済みなら何もしない。失敗時は ERROR 状態にして false。 */
    private fun ensureMocking(): Boolean {
        if (activeProviders.isNotEmpty()) return true
        removeAllTestProviders() // 前回の異常終了の残りを掃除
        val registered = mutableListOf<String>()
        for (name in candidateProviders()) {
            try {
                addTestProvider(name)
                registered += name
            } catch (e: SecurityException) {
                registered.forEach(::removeTestProviderQuietly)
                fail(MOCK_APP_NOT_SELECTED)
                return false
            } catch (e: Exception) {
                // GPS 以外は端末によって登録できないことがある
                Log.w(TAG, "could not add test provider $name: ${e.javaClass.simpleName}")
                if (name == LocationManager.GPS_PROVIDER) {
                    registered.forEach(::removeTestProviderQuietly)
                    fail("failed to add GPS test provider: ${e.message}")
                    return false
                }
            }
        }
        activeProviders = registered
        lastTickAt = SystemClock.elapsedRealtime()
        handler.removeCallbacks(tick)
        handler.post(tick)
        return true
    }

    @Suppress("DEPRECATION")
    private fun addTestProvider(name: String) {
        locationManager.addTestProvider(
            name,
            /* requiresNetwork = */ false,
            /* requiresSatellite = */ name == LocationManager.GPS_PROVIDER,
            /* requiresCell = */ false,
            /* hasMonetaryCost = */ false,
            /* supportsAltitude = */ true,
            /* supportsSpeed = */ true,
            /* supportsBearing = */ true,
            Criteria.POWER_LOW,
            Criteria.ACCURACY_FINE,
        )
        locationManager.setTestProviderEnabled(name, true)
    }

    private fun candidateProviders(): List<String> = buildList {
        add(LocationManager.GPS_PROVIDER)
        add(LocationManager.NETWORK_PROVIDER)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) add(LocationManager.FUSED_PROVIDER)
    }

    private fun removeTestProviderQuietly(name: String) {
        try {
            locationManager.removeTestProvider(name)
        } catch (_: Exception) {
            // 未登録・mock アプリ未選択の場合は何もしない
        }
    }

    private fun removeAllTestProviders() = candidateProviders().forEach(::removeTestProviderQuietly)

    /** mock を解除して通常の位置取得に戻し、サービスを終了する。 */
    private fun stopMocking() {
        handler.removeCallbacks(tick)
        removeAllTestProviders()
        activeProviders = emptyList()
        walk = null
        position = null
        currentStatus = Status(State.IDLE)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        handler.removeCallbacks(tick)
        removeAllTestProviders()
        activeProviders = emptyList()
        currentStatus = Status(State.IDLE)
        super.onDestroy()
    }

    // ---- 位置の更新 ----

    private fun onTick() {
        val now = SystemClock.elapsedRealtime()
        val dt = (now - lastTickAt).coerceAtLeast(0L) / 1000.0
        lastTickAt = now
        val w = walk
        if (w != null) {
            val p = w.advance(dt)
            position = p.point
            bearing = p.bearing
        }
        pushLocation()
        val state = when {
            w == null -> State.HOLDING
            w.arrived -> State.ARRIVED
            w.paused -> State.PAUSED
            else -> State.WALKING
        }
        updateStatus(state)
    }

    private fun publishNow(state: State) {
        pushLocation()
        updateStatus(state)
    }

    private fun pushLocation() {
        val p = position ?: return
        val speed = walk?.currentSpeed ?: 0.0
        for (name in activeProviders) {
            val location = Location(name).apply {
                latitude = p.lat
                longitude = p.lng
                altitude = 0.0
                accuracy = 5f
                this.speed = speed.toFloat()
                this.bearing = this@MockLocationService.bearing.toFloat()
                time = System.currentTimeMillis()
                elapsedRealtimeNanos = SystemClock.elapsedRealtimeNanos()
                verticalAccuracyMeters = 8f
                speedAccuracyMetersPerSecond = 0.5f
                bearingAccuracyDegrees = 10f
            }
            try {
                locationManager.setTestProviderLocation(name, location)
            } catch (e: SecurityException) {
                fail(MOCK_APP_NOT_SELECTED)
                return
            } catch (e: IllegalArgumentException) {
                Log.w(TAG, "setTestProviderLocation failed for $name: ${e.message}")
            }
        }
    }

    private fun updateStatus(state: State) {
        val w = walk
        currentStatus = Status(
            state = state,
            position = position,
            bearing = bearing,
            speedMps = w?.currentSpeed ?: 0.0,
            remainingMeters = w?.remainingMeters ?: 0.0,
            remainingSeconds = w?.remainingSeconds ?: 0.0,
            providers = activeProviders,
        )
        updateNotification()
    }

    /** mock を解除して ERROR 状態にする（誤った成功表示をしない）。 */
    private fun fail(message: String) {
        Log.w(TAG, message)
        handler.removeCallbacks(tick)
        removeAllTestProviders()
        activeProviders = emptyList()
        walk = null
        position = null
        // エラー内容を dumpsys / 通知で確認できるよう、STOP されるまでサービスは残す
        currentStatus = Status(State.ERROR, error = message)
        updateNotification()
    }

    /** mock 開始前の実際の最終位置（mock 由来の値は除外）。 */
    private fun lastKnownRealLocation(): LatLng? {
        if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED &&
            checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) != PackageManager.PERMISSION_GRANTED
        ) return null
        val candidates = listOf(LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER, LocationManager.PASSIVE_PROVIDER)
            .mapNotNull { name ->
                try {
                    locationManager.getLastKnownLocation(name)
                } catch (_: Exception) {
                    null
                }
            }
            .filterNot { it.isMockLocation() }
        return candidates.maxByOrNull { it.elapsedRealtimeNanos }?.let { LatLng(it.latitude, it.longitude) }
    }

    @Suppress("DEPRECATION")
    private fun Location.isMockLocation(): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) isMock else isFromMockProvider

    // ---- 通知 ----

    private fun createNotificationChannel() {
        val channel = NotificationChannel(CHANNEL_ID, "Location simulation", NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun buildNotification(): Notification {
        val s = currentStatus
        val text = when (s.state) {
            State.WALKING -> "Walking: %.0f m left (%s)".format(Locale.US, s.remainingMeters, formatDuration(s.remainingSeconds))
            State.PAUSED -> "Paused: %.0f m left".format(Locale.US, s.remainingMeters)
            State.ARRIVED -> "Arrived. Holding position."
            State.HOLDING -> "Holding a fixed position."
            State.ERROR -> s.error ?: "Error"
            State.IDLE -> "Starting…"
        }
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this, 1, Intent(this, MockLocationService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_menu_mylocation)
            .setContentTitle("Location Lab: location simulation active")
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(s.state != State.ERROR)
            .addAction(Notification.Action.Builder(null, "Stop & restore", stop).build())
            .build()
    }

    private fun enterForeground() {
        val notification = buildNotification()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun updateNotification() {
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, buildNotification())
    }

    // ---- adb dumpsys 用の状態出力 ----

    override fun dump(fd: FileDescriptor?, writer: PrintWriter, args: Array<out String>?) {
        writer.println(formatStatus(currentStatus))
    }

    companion object {
        private const val TAG = "LocationLab"
        private const val CHANNEL_ID = "simulation"
        private const val NOTIFICATION_ID = 1
        private const val TICK_INTERVAL_MS = 1000L

        const val ACTION_SET = "dev.locationlab.action.SET"
        const val ACTION_WALK = "dev.locationlab.action.WALK"
        const val ACTION_PAUSE = "dev.locationlab.action.PAUSE"
        const val ACTION_RESUME = "dev.locationlab.action.RESUME"
        const val ACTION_SPEED = "dev.locationlab.action.SPEED"
        const val ACTION_STOP = "dev.locationlab.action.STOP"

        /** "lat,lng" */
        const val EXTRA_POINT = "point"
        /** "lat,lng;lat,lng;..."（経由地を含む目的地リスト） */
        const val EXTRA_TO = "to"
        /** "lat,lng"（省略時は現在の mock 位置 → 実際の最終位置） */
        const val EXTRA_FROM = "from"
        /** m/s */
        const val EXTRA_SPEED = "speed"

        const val MOCK_APP_NOT_SELECTED =
            "This app is not selected as the mock location app (Developer options > Select mock location app)."
        const val START_UNKNOWN =
            "Start position unknown. Pass a start point, or grant location permission so the last known location can be used."

        @Volatile
        var currentStatus: Status = Status()
            private set

        fun formatDuration(seconds: Double): String {
            val total = seconds.toLong().coerceAtLeast(0)
            return "%d:%02d:%02d".format(Locale.US, total / 3600, (total % 3600) / 60, total % 60)
        }

        /** 1 行 1 項目の key=value 形式。Mac 側 CLI がパースする。 */
        fun formatStatus(s: Status): String = buildString {
            appendLine("state=${s.state}")
            s.position?.let { appendLine("position=%.7f,%.7f".format(Locale.US, it.lat, it.lng)) }
            appendLine("bearing=%.1f".format(Locale.US, s.bearing))
            appendLine("speed_mps=%.2f".format(Locale.US, s.speedMps))
            appendLine("remaining_m=%.1f".format(Locale.US, s.remainingMeters))
            appendLine("remaining_s=%.0f".format(Locale.US, s.remainingSeconds))
            appendLine("providers=${s.providers.joinToString(",")}")
            s.error?.let { appendLine("error=$it") }
        }.trimEnd()
    }
}
