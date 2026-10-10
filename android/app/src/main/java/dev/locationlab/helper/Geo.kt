package dev.locationlab.helper

import kotlin.math.asin
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt

data class LatLng(val lat: Double, val lng: Double) {
    init {
        require(lat.isFinite() && lat in -90.0..90.0) { "latitude out of range: $lat" }
        require(lng.isFinite() && lng in -180.0..180.0) { "longitude out of range: $lng" }
    }

    companion object {
        /** "lat,lng" 形式をパースする。範囲外・不正値は IllegalArgumentException。 */
        fun parse(text: String): LatLng {
            val parts = text.split(',').map { it.trim() }
            require(parts.size == 2) { "expected 'lat,lng': $text" }
            val lat = parts[0].toDoubleOrNull() ?: throw IllegalArgumentException("invalid latitude: ${parts[0]}")
            val lng = parts[1].toDoubleOrNull() ?: throw IllegalArgumentException("invalid longitude: ${parts[1]}")
            return LatLng(lat, lng)
        }

        /** "lat,lng;lat,lng;..." 形式をパースする。 */
        fun parseList(text: String): List<LatLng> =
            text.split(';').map { it.trim() }.filter { it.isNotEmpty() }.map(::parse)
    }
}

object Geo {
    const val EARTH_RADIUS_M = 6_371_008.8

    fun distanceMeters(a: LatLng, b: LatLng): Double {
        val lat1 = Math.toRadians(a.lat)
        val lat2 = Math.toRadians(b.lat)
        val dLat = lat2 - lat1
        val dLng = Math.toRadians(b.lng - a.lng)
        val h = sin(dLat / 2).let { it * it } + cos(lat1) * cos(lat2) * sin(dLng / 2).let { it * it }
        return 2 * EARTH_RADIUS_M * asin(sqrt(h.coerceIn(0.0, 1.0)))
    }

    /** a から b への初期方位角（度, 0..360, 北=0）。 */
    fun bearingDegrees(a: LatLng, b: LatLng): Double {
        val lat1 = Math.toRadians(a.lat)
        val lat2 = Math.toRadians(b.lat)
        val dLng = Math.toRadians(b.lng - a.lng)
        val y = sin(dLng) * cos(lat2)
        val x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dLng)
        return (Math.toDegrees(atan2(y, x)) + 360.0) % 360.0
    }

    /** 大円上で a から b へ fraction (0..1) だけ進んだ点。 */
    fun interpolate(a: LatLng, b: LatLng, fraction: Double): LatLng {
        val f = fraction.coerceIn(0.0, 1.0)
        val delta = distanceMeters(a, b) / EARTH_RADIUS_M
        if (delta < 1e-12) return a
        val lat1 = Math.toRadians(a.lat)
        val lng1 = Math.toRadians(a.lng)
        val lat2 = Math.toRadians(b.lat)
        val lng2 = Math.toRadians(b.lng)
        val sinDelta = sin(delta)
        val ka = sin((1 - f) * delta) / sinDelta
        val kb = sin(f * delta) / sinDelta
        val x = ka * cos(lat1) * cos(lng1) + kb * cos(lat2) * cos(lng2)
        val y = ka * cos(lat1) * sin(lng1) + kb * cos(lat2) * sin(lng2)
        val z = ka * sin(lat1) + kb * sin(lat2)
        val lat = Math.toDegrees(atan2(z, sqrt(x * x + y * y)))
        val lng = Math.toDegrees(atan2(y, x))
        return LatLng(lat.coerceIn(-90.0, 90.0), lng.coerceIn(-180.0, 180.0))
    }
}

/** 経路上の位置と進行方向。 */
data class RoutePosition(val point: LatLng, val bearing: Double)

/** 折れ線経路。進んだ距離から現在位置を求める。 */
class Route(val points: List<LatLng>) {
    private val cumulative: DoubleArray

    init {
        require(points.size >= 2) { "route needs at least 2 points" }
        cumulative = DoubleArray(points.size)
        for (i in 1 until points.size) {
            cumulative[i] = cumulative[i - 1] + Geo.distanceMeters(points[i - 1], points[i])
        }
    }

    val totalMeters: Double get() = cumulative.last()

    fun positionAt(traveledMeters: Double): RoutePosition {
        val d = traveledMeters.coerceIn(0.0, totalMeters)
        // d を含む区間を探す（長さ 0 の区間はスキップ）
        var i = 1
        while (i < points.size - 1 && cumulative[i] < d) i++
        val segStart = points[i - 1]
        val segEnd = points[i]
        val segLen = cumulative[i] - cumulative[i - 1]
        val fraction = if (segLen <= 0.0) 1.0 else (d - cumulative[i - 1]) / segLen
        return RoutePosition(Geo.interpolate(segStart, segEnd, fraction), lastBearingUpTo(i))
    }

    /** 区間 i（points[i-1]→points[i]）の方位。長さ 0 なら直前の有効区間の方位を使う。 */
    private fun lastBearingUpTo(segment: Int): Double {
        for (s in segment downTo 1) {
            if (cumulative[s] - cumulative[s - 1] > 0.0) return Geo.bearingDegrees(points[s - 1], points[s])
        }
        return 0.0
    }
}

/**
 * 一定速度で経路を進むシミュレーター。時間は呼び出し側が与える（テスト容易性のため）。
 */
class WalkSimulator(val route: Route, speedMetersPerSecond: Double) {
    var speedMetersPerSecond: Double = validateSpeed(speedMetersPerSecond)
        set(value) {
            field = validateSpeed(value)
        }

    var traveledMeters: Double = 0.0
        private set

    var paused: Boolean = false

    val arrived: Boolean get() = traveledMeters >= route.totalMeters

    val remainingMeters: Double get() = (route.totalMeters - traveledMeters).coerceAtLeast(0.0)

    val remainingSeconds: Double get() = remainingMeters / speedMetersPerSecond

    /** 経過秒数ぶん進めて現在位置を返す。一時停止中・到着後は位置が変わらない。 */
    fun advance(elapsedSeconds: Double): RoutePosition {
        require(elapsedSeconds >= 0.0) { "elapsed must be >= 0" }
        if (!paused && !arrived) {
            traveledMeters = (traveledMeters + speedMetersPerSecond * elapsedSeconds).coerceAtMost(route.totalMeters)
        }
        return route.positionAt(traveledMeters)
    }

    /** 現在、実際に移動中の速度（一時停止中・到着後は 0）。 */
    val currentSpeed: Double get() = if (paused || arrived) 0.0 else speedMetersPerSecond

    companion object {
        const val MIN_SPEED_MPS = 0.1
        const val MAX_SPEED_MPS = 50.0
        /** 一般的な徒歩速度 (約 5 km/h)。 */
        const val DEFAULT_WALK_SPEED_MPS = 1.4

        fun validateSpeed(speed: Double): Double {
            require(speed.isFinite() && speed in MIN_SPEED_MPS..MAX_SPEED_MPS) {
                "speed must be $MIN_SPEED_MPS..$MAX_SPEED_MPS m/s: $speed"
            }
            return speed
        }
    }
}
