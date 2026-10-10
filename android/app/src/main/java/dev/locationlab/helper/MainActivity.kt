package dev.locationlab.helper

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.InputType
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import java.util.Locale

/**
 * 端末上で操作するための最小 UI。Mac からの adb 操作と同じ Intent をサービスに送る。
 */
class MainActivity : Activity() {

    private val handler = Handler(Looper.getMainLooper())
    private lateinit var statusView: TextView
    private lateinit var destinationInput: EditText
    private lateinit var startInput: EditText
    private lateinit var speedInput: EditText

    private val refresh = object : Runnable {
        override fun run() {
            renderStatus()
            handler.postDelayed(this, 1000L)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(buildLayout())
        requestPermissionsIfNeeded()
    }

    override fun onResume() {
        super.onResume()
        handler.post(refresh)
    }

    override fun onPause() {
        handler.removeCallbacks(refresh)
        super.onPause()
    }

    private fun buildLayout(): ScrollView {
        val pad = (16 * resources.displayMetrics.density).toInt()
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad, pad, pad)
        }
        fun label(text: String) = TextView(this).apply { this.text = text; setPadding(0, pad / 2, 0, 0) }
        fun input(hint: String, type: Int) = EditText(this).apply {
            this.hint = hint
            inputType = type
            isSingleLine = true
        }
        fun button(text: String, onClick: () -> Unit) = Button(this).apply {
            this.text = text
            setOnClickListener { onClick() }
        }

        val coordType = InputType.TYPE_CLASS_TEXT
        destinationInput = input("35.681236,139.767125", coordType)
        startInput = input("空欄なら現在位置から", coordType)
        speedInput = input("5.0", InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_FLAG_DECIMAL).apply {
            setText("5.0")
        }
        statusView = TextView(this).apply {
            typeface = android.graphics.Typeface.MONOSPACE
            setPadding(0, pad, 0, pad)
        }

        root.addView(TextView(this).apply { text = "Location Lab"; textSize = 22f })
        root.addView(statusView)
        root.addView(label("目的地 (緯度,経度; 経由地は ; 区切り)"))
        root.addView(destinationInput)
        root.addView(label("出発地 (任意)"))
        root.addView(startInput)
        root.addView(label("速度 km/h (徒歩 ≒ 4〜5)"))
        root.addView(speedInput)
        root.addView(button("徒歩で移動開始") { startWalk() })
        root.addView(button("目的地に即時固定") { setFixed() })
        root.addView(button("一時停止") { send(MockLocationService.ACTION_PAUSE) })
        root.addView(button("再開") { send(MockLocationService.ACTION_RESUME) })
        root.addView(button("停止して通常位置に戻す") { send(MockLocationService.ACTION_STOP) })
        root.addView(label("初回設定: 開発者向けオプション →「仮の現在地情報アプリを選択」でこのアプリを選択"))
        root.addView(button("開発者向けオプションを開く") {
            startActivity(Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS))
        })
        return ScrollView(this).apply { addView(root, MATCH_PARENT, WRAP_CONTENT) }
    }

    private fun startWalk() {
        val speedKmh = speedInput.text.toString().trim().toDoubleOrNull()
        if (speedKmh == null) return toast("速度が不正です")
        val speedMps = speedKmh / 3.6
        val to = destinationInput.text.toString().trim()
        val from = startInput.text.toString().trim()
        try {
            WalkSimulator.validateSpeed(speedMps)
            require(LatLng.parseList(to).isNotEmpty()) { "目的地を入力してください" }
            if (from.isNotEmpty()) LatLng.parse(from)
        } catch (e: IllegalArgumentException) {
            return toast(e.message ?: "入力が不正です")
        }
        send(MockLocationService.ACTION_WALK) {
            putExtra(MockLocationService.EXTRA_TO, to)
            putExtra(MockLocationService.EXTRA_SPEED, speedMps.toString())
            if (from.isNotEmpty()) putExtra(MockLocationService.EXTRA_FROM, from)
        }
    }

    private fun setFixed() {
        val point = destinationInput.text.toString().trim()
        try {
            LatLng.parse(point)
        } catch (e: IllegalArgumentException) {
            return toast(e.message ?: "座標が不正です")
        }
        send(MockLocationService.ACTION_SET) { putExtra(MockLocationService.EXTRA_POINT, point) }
    }

    private fun send(action: String, extras: Intent.() -> Unit = {}) {
        val intent = Intent(this, MockLocationService::class.java).setAction(action).apply(extras)
        startForegroundService(intent)
    }

    private fun renderStatus() {
        val s = MockLocationService.currentStatus
        val lines = mutableListOf("状態: ${s.state}")
        s.position?.let { lines += "位置: %.6f, %.6f".format(Locale.US, it.lat, it.lng) }
        if (s.state == MockLocationService.State.WALKING || s.state == MockLocationService.State.PAUSED) {
            lines += "残り: %.0f m / %s".format(Locale.US, s.remainingMeters, MockLocationService.formatDuration(s.remainingSeconds))
            lines += "速度: %.1f km/h".format(Locale.US, s.speedMps * 3.6)
        }
        s.error?.let { lines += "エラー: $it" }
        if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED) {
            lines += "※位置情報の権限がないため、出発地の入力が必要です"
        }
        statusView.text = lines.joinToString("\n")
    }

    private fun requestPermissionsIfNeeded() {
        val wanted = buildList {
            add(Manifest.permission.ACCESS_FINE_LOCATION)
            add(Manifest.permission.ACCESS_COARSE_LOCATION)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(Manifest.permission.POST_NOTIFICATIONS)
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (wanted.isNotEmpty()) requestPermissions(wanted.toTypedArray(), 1)
    }

    private fun toast(message: String) = Toast.makeText(this, message, Toast.LENGTH_LONG).show()
}
