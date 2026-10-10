# location-lab

Mac に接続した Android / iOS 端末の位置情報をシミュレーションするツールです。
Android は公式の mock location（テストプロバイダ）機構、iOS は Apple の開発者向け位置シミュレーション機能を使います。
地図で目的地を指定すると、現在地から目的地まで **徒歩の速度で少しずつ移動** させることができます。

| ディレクトリ | 内容 |
|---|---|
| `android/` | 端末にインストールする補助アプリ（Kotlin、外部ライブラリ依存なし） |
| `desktop/` | Mac の GUI アプリ（Tauri 2。地図で操作。Android / iOS 両対応） |
| `mac/locationlab.py` | Mac から操作する CLI（Python 3 標準ライブラリのみ） |

```
 Mac (GUI / CLI) ──USB / Wi-Fi (adb)────────▶ Android: Location Lab アプリ ──▶ mock location
 Mac (GUI)       ──USB (pymobiledevice3)────▶ iPhone / iPad: 開発者向け位置シミュレーション
```

iOS だけを使う場合は、下の「セットアップ」の Android 向け手順（2・3・5）は不要です。「[iOS（iPhone / iPad）で使う](#iosiphone--ipadで使う)」を参照してください。

---

## 動作確認状況

2026-10-10 時点の状況です。

| 対象 | 状況 | 確認内容 |
|---|---|---|
| Android（エミュレータ） | ✅ 確認済み | Android 14 エミュレータで、固定・徒歩移動・一時停止・再開・速度変更・停止、終了時の自動解除、Wi-Fi 接続（CLI / GUI） |
| Android（実機） | ✅ 一部確認済み | Galaxy Z Fold7（Android 16）に Wi-Fi で接続し、端末セットアップ・徒歩移動・停止（通常位置への復帰） |
| iOS（実機） | ⚠️ 未確認 | 実機を使ったテストは未実施です（来週以降に実施予定）。現時点では、pymobiledevice3 の出力を模したフェイクで GUI の操作の流れと単体テストのみ確認しています |

iOS を試す場合は、問題が起きる可能性がある前提で使ってください。不具合があれば、GUI に表示されたエラーメッセージを添えて Issue で報告してください。

## セットアップ

初回は 20〜30 分ほどかかります（ダウンロード時間を含む）。

### 1. Mac に必要なツールを入れる

[Homebrew](https://brew.sh/) が入っている前提です。

```sh
# adb（端末との通信）
brew install --cask android-platform-tools

# Android アプリのビルドに必要な JDK 17 と Android SDK
brew install --cask temurin@17 android-commandlinetools

# GUI アプリのビルドに必要な Node.js
brew install node

# GUI アプリのビルドに必要な Rust（公式インストーラ）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

Android SDK の場所を環境変数に設定し、必要なパッケージを入れます。

```sh
# zsh の場合
echo 'export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools' >> ~/.zshrc
source ~/.zshrc
# fish の場合
# set -Ux ANDROID_HOME /opt/homebrew/share/android-commandlinetools

yes | sdkmanager --licenses
sdkmanager "platforms;android-34" "build-tools;34.0.0"
```

> Android Studio が入っている場合は、`temurin@17`・`android-commandlinetools`・`sdkmanager` の手順は不要です。
> `ANDROID_HOME` を `~/Library/Android/sdk` に設定してください。

新しいターミナルを開いて、次のコマンドがすべてバージョンを表示すれば OK です。

```sh
adb version && java -version && cargo --version && node --version
```

### 2. Android 端末の準備（初回のみ）

1. **開発者向けオプションを有効にする**
   「設定 → デバイス情報（端末情報）→ ビルド番号」を 7 回タップします。
   メーカーによっては「ソフトウェア情報」の中にあります。
2. **USB デバッグを有効にする**
   「設定 → システム → 開発者向けオプション → USB デバッグ」を ON にします。
3. **Mac と USB で接続する**
   端末に「USB デバッグを許可しますか？」と表示されたら、「このパソコンからの USB デバッグを常に許可する」にチェックして許可します。
4. **接続を確認する**
   次のコマンドで `device` と表示されれば OK です。`unauthorized` と出る場合は、手順 3 の許可を確認してください。

   ```sh
   adb devices
   # List of devices attached
   # XXXXXXXX   device
   ```

### 3. Android アプリをビルドする

```sh
git clone https://github.com/andoshin11/location-lab.git
cd location-lab/android
./gradlew assembleDebug
# → android/app/build/outputs/apk/debug/app-debug.apk ができます
cd ..
```

### 4. Mac の GUI アプリをビルドする

```sh
cd desktop
npm install
npm run build
# → desktop/src-tauri/target/release/bundle/macos/Location Lab.app ができます
open "src-tauri/target/release/bundle/macos/Location Lab.app"
cd ..
```

`Location Lab.app` は「アプリケーション」フォルダにドラッグして使っても構いません。
署名していないため、初回起動時に警告が出た場合は Finder で右クリック →「開く」を選んでください。

開発中は `npm run dev` でビルドせずに起動できます。

### 5. 端末にアプリを入れて設定する（GUI から）

1. GUI の左上「端末」に、接続した端末が表示されていることを確認します。
2. 「端末セットアップ」を開き、APK のパスに次を入力して「セットアップ実行」を押します。

   ```
   /path/to/location-lab/android/app/build/outputs/apk/debug/app-debug.apk
   ```

   これで次の 3 つがまとめて行われます。
   - アプリのインストール
   - 位置情報の権限付与（現在地を出発地として使うため）
   - 「仮の現在地情報アプリ」への設定

> CLI では `./mac/locationlab.py setup --apk android/app/build/outputs/apk/debug/app-debug.apk` で同じことができます。
> 端末上で手動設定する場合は「開発者向けオプション → 仮の現在地情報アプリを選択 → Location Lab」です。

これでセットアップは完了です。

### （任意）Wi-Fi で接続する

Android 11 以降なら、USB の代わりに Wi-Fi（ワイヤレスデバッグ）でも操作できます。Mac と端末を同じ Wi-Fi に接続してください。
初回の「端末セットアップ」（手順 5）は Wi-Fi 接続のままでも実行できます。

1. 端末で「開発者向けオプション → ワイヤレスデバッグ」を ON にします。
2. **ペア設定（初回のみ）**
   - 端末で「ペア設定コードによるデバイスのペア設定」をタップします。
   - GUI の「ワイヤレス接続（Wi-Fi）」を開き、「1. ペア設定」に端末に表示された「IP アドレスとポート」と 6 桁のコードを入力して「ペア設定」を押します。
3. **接続**
   - 「2. 接続」に、ワイヤレスデバッグ画面のトップに表示されている「IP アドレスとポート」を入力して「接続」を押します。ペア設定用とは別のポートです。
   - 端末欄に「〜 · Wi-Fi」と表示されれば接続完了です。あとは USB と同じように操作できます。

補足:
- 端末の再起動や Wi-Fi の切り替えで、接続用のポートが変わったりワイヤレスデバッグが OFF になったりします。その場合は手順 1 と 3 をやり直してください（ペア設定は不要）。
- 最後に接続したアドレスは GUI が覚えています。
- 「選択中のワイヤレス端末を切断」で切断できます。切断しても、端末上の移動・固定は続きます。止めたい場合は、先に「停止して通常位置に戻す」を押してください。
- CLI では `adb pair <IP:ポート> <コード>` と `adb connect <IP:ポート>` で接続したあと、`-s <IP:ポート>` を付けて操作します。

---

## iOS（iPhone / iPad）で使う

iOS には端末側のアプリを入れません。Mac の GUI が、Apple の開発者向け位置シミュレーション機能を [pymobiledevice3](https://github.com/doronz88/pymobiledevice3) 経由で操作します。

- 対応: iOS 17 以降（DVT の位置シミュレーション）と iOS 16 以下（`com.apple.dt.simulatelocation`）。接続は USB のみです。
- 徒歩移動は Mac 側で 1 秒ごとの地点を計算して端末へ送ります。そのため、**移動中は Mac と USB で接続したまま**にしてください。

### 1. Mac に pymobiledevice3 を入れる

上の「1. Mac に必要なツールを入れる」の Node.js・Rust（GUI のビルド用）に加えて、次を実行します。

```sh
brew install uv
uv tool install pymobiledevice3
uv tool update-shell   # ~/.local/bin を PATH に追加（新しいターミナルで有効）
pymobiledevice3 version
```

GUI は PATH、`~/.local/bin`、Homebrew の場所から `pymobiledevice3` を自動で探します。見つからない場合は、環境変数 `LOCATIONLAB_PMD3` にパスを指定してください。

> pymobiledevice3 は GPL-3.0-or-later です。本リポジトリには含めず、GUI からは別プロセスのコマンドとして呼び出しています。

### 2. iPhone / iPad の準備（初回のみ）

1. **USB で Mac に接続**し、端末に「このコンピュータを信頼しますか？」と表示されたら「信頼」を選んでパスコードを入力します。
2. **デベロッパモードを ON にする**（iOS 16 以降）
   「設定 → プライバシーとセキュリティ → デベロッパモード」を ON にし、再起動後の確認で「オンにする」を選びます。
   - 項目が表示されない場合は、先に GUI の「端末セットアップ」を一度実行すると表示されるようになります。
3. 端末のロックを解除しておきます（ロック中は操作に失敗することがあります）。

### 3. GUI から端末セットアップ

1. GUI を起動し、端末欄に「（端末名） · iOS 18.x」のように表示されることを確認します。
2. 「端末セットアップ」→「セットアップ実行」を押します。
   - デベロッパモードを確認し、位置シミュレーションに必要なデベロッパディスクイメージを端末にマウントします。
   - 初回はイメージのダウンロードで数分かかることがあります。端末を再起動したあとは、もう一度実行してください。

### iOS での使い方の違い

- **初回は出発地の指定が必要です。** iOS では Mac から端末の実際の現在地を取得できないため、「最初の点を出発地にする」にチェックして、地図で出発地 → 目的地の順にクリックしてください。2 回目以降は、現在のシミュレーション位置から出発できます。
- 各操作は端末との接続確立に数秒〜数十秒かかることがあります（「iPhone / iPad と通信しています…」と表示されます）。
- 位置は Mac 上の pymobiledevice3 プロセスが保持しています。
  - 「終了時にシミュレーションを解除」が ON（既定）なら、GUI 終了時に解除します。
  - OFF の場合や GUI が強制終了した場合は、バックグラウンドのプロセスが位置を保持し続けます。次に GUI を起動して「停止して通常位置に戻す」を押すと、残ったプロセスも終了して解除されます。
- 手動で解除する場合: `pymobiledevice3 developer dvt simulate-location clear`（iOS 16 以下は `developer simulate-location clear`）。端末の再起動でも解除されます。

## 使い方（GUI）

1. **目的地を指定する**
   地図をクリックすると目的地が追加されます。続けてクリックした地点は経由地になり、その順に歩きます。
   - 座標を直接入力する場合は「緯度,経度」欄に入力して「追加」を押します。
   - GPX ファイルの経路を使う場合は「GPX 読込」を押します。
   - 出発地も指定したい場合は「最初の点を出発地にする」にチェックします。チェックしなければ端末の現在地から出発します。
2. **速度を選ぶ**
   既定は 5 km/h（徒歩）です。距離と所要時間が「経路」欄に表示されます。
3. **「徒歩で移動開始」を押す**
   端末の位置が 1 秒ごとに更新され、地図上の青いマーカーが動きます。
   - 「一時停止」「再開」や、移動中の速度「変更」ができます。
   - 到着すると、その場所に留まります。
4. **「停止して通常位置に戻す」を押す**
   シミュレーションを解除し、端末は本来の位置情報に戻ります。

その他の操作:
- **最後の点に固定**: 指定した地点に位置を固定します。現在のシミュレーション位置から 100 m を超える瞬間移動になる場合は、確認ダイアログが出ます。
- **終了時にシミュレーションを解除**: ON（既定）なら、GUI を終了したときに自動で解除されます。OFF にすると、GUI を閉じても端末上で移動・固定が続きます。
- **端末側からの停止**: 通知の「Stop & restore」や、端末の Location Lab アプリの画面からも停止できます。

## 使い方（CLI）

```sh
./mac/locationlab.py walk 35.681236,139.767125 --follow        # 現在位置から東京駅まで 5 km/h で歩く
./mac/locationlab.py walk 35.6895,139.6917 35.6812,139.7671    # 経由地を順に歩く
./mac/locationlab.py walk --gpx route.gpx --speed 4             # GPX の経路を 4 km/h で
./mac/locationlab.py walk 35.68,139.76 --from 35.69,139.70      # 出発地を指定
./mac/locationlab.py pause | resume | status
./mac/locationlab.py speed 3.5                                  # 速度変更 (km/h)
./mac/locationlab.py set 35.681236,139.767125                   # 固定（100 m 超の瞬間移動は --force が必要）
./mac/locationlab.py stop                                       # 解除して実際の位置に戻す
```

複数の端末を接続している場合は `-s <serial>` を付けます。

---

## 仕組み・仕様

- 端末側アプリは Foreground Service として、gps / network / fused（Android 12 以降）のテストプロバイダに 1 秒ごとに位置・進行方向・速度を設定します。
- 移動の出発地は、次の順で決まります。
  1. 指定した出発地
  2. 現在のシミュレーション位置
  3. シミュレーション開始前の実際の最終位置
- 状態は `IDLE` / `HOLDING` / `WALKING` / `PAUSED` / `ARRIVED` / `ERROR` のいずれかです。`adb shell dumpsys activity service dev.locationlab.helper/.MockLocationService` で取得できます。
- mock 位置アプリに設定されていない場合などは `ERROR` になり、GUI / CLI は成功と表示しません。
- 停止は、サービス終了（テストプロバイダ削除）を確認してから完了と表示します。強制終了などで解除されていない場合も、もう一度停止を実行すれば残ったテストプロバイダを削除します。
- 位置履歴は保存しません。座標はログ（logcat）にも出力しません。

### Mac GUI について

- Electron ではなく Tauri を採用しています。OS 標準の WebView を使い Chromium を同梱しないため、アプリサイズとメモリ使用量が小さくなります。
  - 実測で .app は約 3.3 MB、移動中の本体プロセスは約 80 MB・CPU ほぼ 0% でした（WebKit プロセスは別）。
  - ウィンドウ非表示中は、状態取得の間隔を 5 秒に落とします。
- 地図は OpenStreetMap の標準タイルを使っています。無料で API キーも不要です。[タイル利用ポリシー](https://operations.osmfoundation.org/policies/tiles/)の範囲で利用してください。
- adb は PATH、`~/Library/Android/sdk/platform-tools`、Homebrew の場所から自動で探します。
  - 見つからない場合は、環境変数 `LOCATIONLAB_ADB` に adb のパスを指定してください。
  - Finder から起動した .app は、シェルの PATH を引き継ぎません。

## 制約

- iOS 17 以降では、アプリはソフトウェアによる位置シミュレーションかどうか（`CLLocationSourceInformation.isSimulatedBySoftware`）を判定できます。Android の mock フラグと同様、アプリ側の検出の回避は対象外です。
- Android の mock location 機構を使うため、受け取った位置には mock フラグ（`Location.isMock()`）が付きます。アプリ側がこれを検出して拒否・制限する場合があり、その回避は対象外です。各アプリの利用規約は利用者の責任で確認してください。
- Google Play 開発者サービスの Fused Location Provider に反映されるかどうかは、端末・バージョンによって異なります。
- 端末上で動き続ける Foreground Service なので、Mac との接続が切れても移動は続きます。

## トラブルシューティング

| 症状 | 対処 |
|---|---|
| GUI の端末欄に「端末が接続されていません」と出る | `adb devices` で `device` と出るか確認します。USB ケーブルがデータ通信に対応しているか、USB デバッグが ON かを確認し、↻ ボタンで再読込します。 |
| Wi-Fi で「接続できませんでした」と出る | 端末と Mac が同じ Wi-Fi か、ワイヤレスデバッグが ON か確認します。入力したのがペア設定用ではなく接続用のポートか確認します（再起動などで変わります）。 |
| 「ペア設定に失敗しました」と出る | ペア設定の画面を開いたまま、表示中のコードとポートで再度試します（画面を閉じるとコードが無効になります）。 |
| 端末が `unauthorized` になる | 端末の画面に出ている「USB デバッグを許可」を承認します。 |
| 「adb に接続できません」「adb が見つかりません」と出る | `brew install --cask android-platform-tools` を実行します。別の場所の adb を使う場合は `LOCATIONLAB_ADB` を設定します。 |
| 「not selected as the mock location app」エラー | 「端末セットアップ」を実行するか、開発者向けオプションの「仮の現在地情報アプリ」で Location Lab を選択します。 |
| 「Start position unknown」エラー | 端末がまだ現在地を取得していません。「最初の点を出発地にする」で出発地を指定するか、端末で一度地図アプリなどを開いて現在地を取得します。 |
| adb が頻繁に落ちて `device offline` になる | macOS の新しいバージョンで、mDNS 周りのクラッシュを確認しています。`adb kill-server` の後に `ADB_MDNS=0 ADB_MDNS_OPENSCREEN=0 adb start-server` で起動し直します。 |
| iOS 端末が端末欄に出ない | USB で接続し、端末で「信頼」を選んだか確認します。`pymobiledevice3 usbmux list` に表示されるか確認し、↻ で再読込します。 |
| 「pymobiledevice3 が必要です」と出る | 「iOS で使う」の手順 1 でインストールします。別の場所にある場合は `LOCATIONLAB_PMD3` を設定します。 |
| iOS で「デベロッパモード」のエラー | 端末の「設定 → プライバシーとセキュリティ → デベロッパモード」を ON にして再起動します。 |
| iOS で「デベロッパディスクイメージ」「端末セットアップを実行」のエラー | 「端末セットアップ」を実行します。端末の再起動後は毎回必要です。 |
| iOS で「端末からの応答がありません（タイムアウト）」 | 端末のロックを解除し、USB ケーブルを挿し直してから再実行します。 |
| `./gradlew` で SDK が見つからない | `echo $ANDROID_HOME` で環境変数が設定されているか確認します。 |
| `npm run build` で `cargo` が見つからない | 新しいターミナルを開くか、`source "$HOME/.cargo/env"` を実行します。 |

## アンインストール

```sh
./mac/locationlab.py stop                 # シミュレーション中なら解除
adb uninstall dev.locationlab.helper      # 端末から削除
```

不要であれば、端末の「開発者向けオプション」で USB デバッグを OFF にしてください。

## テスト

```sh
cd android && ./gradlew testDebugUnitTest   # 座標・経路・速度計算
python3 -m unittest discover -s mac/tests  # CLI（座標検証・GPX・コマンド）
cd desktop && npm test                      # GUI の座標計算・GPX パース
cd desktop/src-tauri && cargo test          # GUI の adb / iOS 連携（入力検証・状態パース・経路計算・フェイク pymobiledevice3 での操作）
```
