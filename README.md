# location-lab

Mac に接続した Android 端末の位置情報を、Android 公式の mock location（テストプロバイダ）機構でシミュレーションするツールです。

- `android/` — 端末側の補助アプリ（Kotlin、外部ライブラリ依存なし）
- `mac/locationlab.py` — Mac から adb 経由で操作する CLI（Python 3 標準ライブラリのみ）

## できること

- 指定座標への固定（`set`）
- **徒歩移動シミュレーション**（`walk`）: 現在位置から目的地（複数の経由地・GPX 可）まで、指定速度（既定 5 km/h）で 1 秒ごとに位置を更新しながら移動。進行方向・速度も Location に設定します。
  - 出発地は「現在のシミュレーション位置」→「mock 開始前の実際の最終位置」→ `--from` の指定、の順で決まります。
  - 一時停止・再開・速度変更に対応。到着後はその地点を保持します。
- `set` で現在のシミュレーション位置から 100 m 超の瞬間移動になる場合は拒否します（`--force` で許可）。
- 停止（`stop`）でテストプロバイダを削除し、通常の位置取得に戻します。

## 必要なもの

- Android 8.0 (API 26) 以上の端末、USB デバッグ有効
- Mac: `adb`（`brew install --cask android-platform-tools`）、Python 3.9+
- ビルドする場合: JDK 17、Android SDK (platform 34)

## セットアップ

```sh
# 1. APK をビルド
cd android && ./gradlew assembleDebug && cd ..

# 2. インストールして権限付与 + 「仮の現在地情報アプリ」に設定
./mac/locationlab.py setup --apk android/app/build/outputs/apk/debug/app-debug.apk
```

`setup` は `adb shell appops set ... android:mock_location allow` を実行します。端末上で手動設定する場合は「開発者向けオプション → 仮の現在地情報アプリを選択 → Location Lab」です。

## 使い方

```sh
./mac/locationlab.py walk 35.681236,139.767125 --follow        # 現在位置から東京駅まで 5 km/h で歩く
./mac/locationlab.py walk 35.6895,139.6917 35.6812,139.7671    # 経由地を順に歩く
./mac/locationlab.py walk --gpx route.gpx --speed 4             # GPX の経路を 4 km/h で
./mac/locationlab.py walk 35.68,139.76 --from 35.69,139.70      # 出発地を指定
./mac/locationlab.py pause | resume | status
./mac/locationlab.py speed 3.5                                  # 速度変更 (km/h)
./mac/locationlab.py set 35.681236,139.767125                   # 固定（初回、または近距離のみ）
./mac/locationlab.py stop                                       # 解除して実際の位置に戻す
```

複数端末接続時は `-s <serial>` を付けます。端末上のアプリ画面からも同じ操作ができます。

## 状態とエラー

- 状態: `IDLE` / `HOLDING` / `WALKING` / `PAUSED` / `ARRIVED` / `ERROR`（`adb shell dumpsys activity service dev.locationlab.helper/.MockLocationService` で取得）
- mock 位置アプリに選択されていない等で失敗した場合は `ERROR` になり、CLI は成功表示をせず終了コード 1 を返します。テストプロバイダは解除されます。
- `stop` はサービス終了（テストプロバイダ削除）を確認してから完了を表示します。サービスが強制終了された等で解除されていない場合も、もう一度 `stop` を実行すれば残ったテストプロバイダを削除します。

## プライバシー

位置履歴は保存しません。座標はログ（logcat）にも出力しません。

## 制約

- Android の mock location 機構を使うため、受け取った位置には mock フラグ（`Location.isMock()`）が付きます。アプリ側がこれを検出して拒否・制限する場合があり、その回避は対象外です。各アプリの利用規約は利用者の責任で確認してください。
- Google Play 開発者サービスの Fused Location Provider への反映は端末・バージョンに依存します（本アプリはプラットフォームの gps / network / fused(API 31+) テストプロバイダに位置を設定します）。
- 端末上で動き続ける Foreground Service なので、Mac との接続が切れても移動は継続します。終了は `stop`、通知の「Stop & restore」、またはアプリの「停止」ボタンで行います。

## テスト

```sh
cd android && ./gradlew testDebugUnitTest   # 座標・経路・速度計算
python3 -m unittest discover -s mac/tests  # CLI（座標検証・GPX・コマンド）
```
