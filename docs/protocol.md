# KDE Connect protocol reference (v8), as implemented upstream

Verified against upstream source on 2026-11-11:

- `A:` = `kdeconnect-android` @ `5238479`, paths relative to `src/main/java/org/kde/kdeconnect/`
- `K:` = `kdeconnect-kde` @ `ff991ea`, paths relative to the repo root

Every claim cites `repo:path` and, where useful, the function. "Phone" = stock Android app, "desktop" = kdeconnect-kde,
"we" = Tether. Anything not cited here is not verified.

## 0. Wire format

- One packet = one line of compact JSON terminated by `\n`:
  `{"id": <i64 ms>, "type": "<str>", "body": {...}, "payloadSize": <i64>?, "payloadTransferInfo": {"port": <int>}?}`
  (`K:core/networkpacket.cpp` `serialize`; `A:NetworkPacket.kt` `SerializedPacket`).
- Phone parser: `type` and `body` are required (no default), `id` defaults to 0, unknown top-level keys ignored
  (`A:NetworkPacket.kt` `SerializedPacket`; `A:helpers/JsonHelper.kt` `ignoreUnknownKeys = true`).
- Phone: if `payloadSize` is present, `payloadTransferInfo` **must** be present, else `SerializationException`
  (`A:NetworkPacket.kt` `unserialize`). Phone only writes `payloadSize`/`payloadTransferInfo` when payload size != 0
  (`A:NetworkPacket.kt` `serialize`, `hasPayload`).
- Phone body getters are lenient: `getInt`/`getLong` parse strings and doubles, `getBoolean` accepts `"true"`/`"false"`
  strings, `getString` returns any primitive's text (`A:NetworkPacket.kt`).
- Phone max packet line on the main link: 32 MiB; longer lines are skipped (`A:backends/lan/LanLink.kt` `MAX_PACKET_SIZE`,
  `startListening`). Max identity line: 512 KiB (`A:backends/lan/LanLinkProvider.kt` `MAX_IDENTITY_PACKET_SIZE`).
- Protocol version is 8 on both sides (`A:helpers/DeviceHelper.kt` `PROTOCOL_VERSION`; `K:core/networkpacket.cpp`
  `s_protocolVersion`).

## 1. Discovery

### 1.1 UDP

- UDP port **1716**, both listen and send to it (`A:backends/lan/LanLinkProvider.kt` `UDP_PORT`, `MIN_PORT`;
  `K:core/backends/lan/lanlinkprovider.h` `UDP_PORT`).
- Broadcast target `255.255.255.255` plus user-configured custom IPs (`A:...LanLinkProvider.kt`
  `broadcastUdpIdentityPacket`; `K:...lanlinkprovider.cpp` `getBroadcastAddresses`). Desktop sends from every up IPv4
  interface address (`K:...lanlinkprovider.cpp` `sendUdpDiscoveryPacket`).
- Phone only broadcasts when the current network is "trusted" (default: all networks allowed) and, on newer Android,
  has `ACCESS_LOCAL_NETWORK` (`A:...LanLinkProvider.kt` `broadcastUdpIdentityPacket`; `A:helpers/TrustedNetworkHelper.kt`
  `isTrustedNetwork`).
- Phone UDP identity = full identity packet + `tcpPort` (`A:...LanLinkProvider.kt` `sendUdpIdentityPacket`;
  `A:DeviceInfo.kt` `toIdentityPacket`). Desktop's UDP packet is minimal: `deviceId`, `deviceName`, `protocolVersion`,
  `tcpPort` (`K:core/deviceinfo.h` `toUdpDiscoveryPacket`).
- Full identity (`kdeconnect.identity`) body (`A:DeviceInfo.kt` `toIdentityPacket`; `K:core/deviceinfo.h` `toIdentityPacket`):

  | field | type | notes |
  | --- | --- | --- |
  | `deviceId` | string | must match `^[a-zA-Z0-9_-]{32,38}$` (`A:DeviceInfo.kt` `DEVICE_ID_REGEX`; `K:core/deviceinfo.h` `isValidDeviceId`) |
  | `deviceName` | string | required non-blank on phone after filtering (below) |
  | `deviceType` | string | `phone`/`tablet`/`desktop`/`laptop`/`tv`; unknown = desktop (`A:DeviceInfo.kt` `DeviceType.fromString`) |
  | `protocolVersion` | int | 8 |
  | `incomingCapabilities` | string[] | packet types we can receive |
  | `outgoingCapabilities` | string[] | packet types we may send |
  | `tcpPort` | int | UDP only; 1716..1764 |

- Phone validity check for **every** identity (UDP, plaintext TCP, and post-TLS): type is `kdeconnect.identity`,
  filtered `deviceName` non-blank, `deviceId` matches regex (`A:DeviceInfo.kt` `isValidIdentityPacket`). Desktop's UDP check
  only requires the id (`K:core/deviceinfo.h` `isValidUdpDiscoveryPacket`) - **so always send `deviceName`**.
- Device name filter: remove `"',;:.!?()[]<>`, trim, keep first 32 chars (`A:helpers/DeviceHelper.kt`
  `filterInvalidCharactersFromDeviceNameAndLimitLength`; `K:core/deviceinfo.h` `filterName`).
- Device id generation: phone = random UUID without dashes (32 hex) (`A:helpers/DeviceHelper.kt` `initializeDeviceId`);
  desktop = `QUuid::createUuid().toString(Id128).toLower()` (32 hex), stored as the cert CN, and desktop's id **is read
  back from the cert CN** (`K:core/kdeconnectconfig.cpp` `generateCertificate`, `deviceId`).
- Phone TCP listener: first free port in **1716..1764** (`A:...LanLinkProvider.kt` `openServerSocketOnFreePort`, `MAX_PORT`);
  desktop same (`K:...lanlinkprovider.h` `MIN_TCP_PORT`/`MAX_TCP_PORT`).
- Phone ignore rules on receiving an identity (`A:...LanLinkProvider.kt` `udpPacketReceived`, `unserializeReceivedIdentityPacket`):
  - source address not private (loopback, site-local, link-local, CGNAT 100.64/10, IPv6 ULA) and not a custom device
    -> dropped (`isAllowedAddress`; `A:helpers/NetworkHelper.kt` `isPrivateAddress`);
  - rate limit: a second UDP **or TCP** from the same IP within 1000 ms is dropped (shared map
    `lastConnectionTimeByIp`), and a second identity with the same `deviceId` within 1000 ms is dropped
    (`rateLimitByIp`, `rateLimitByDeviceId`, `MILLIS_DELAY_BETWEEN_CONNECTIONS_TO_SAME_DEVICE`);
  - own `deviceId` -> ignored;
  - untrusted device while on an untrusted network -> ignored;
  - `tcpPort` outside 1716..1764 -> ignored (missing `tcpPort` defaults to 1716);
  - trusted device announcing a lower `protocolVersion` than stored -> refused (`isProtocolDowngrade`);
  - more than 42 unpaired links -> refused (`MAX_UNPAIRED_CONNECTIONS`).
- Desktop ignores `protocolVersion < 8` entirely (`K:...lanlinkprovider.cpp` `deviceDiscovered`, `tcpPacketReceived`).
  Phone still talks to < 8 peers without the secure identity step (`A:...LanLinkProvider.kt` `identityPacketReceived`).

### 1.2 mDNS

- Service type `_kdeconnect._udp`, instance name = deviceId, port = the TCP listener port, TXT keys `id`, `name`,
  `type`, `protocol` (`A:backends/lan/MdnsDiscovery.kt` `createNsdServiceInfo`; `K:core/backends/lan/avahidiscovery.cpp`;
  `K:core/backends/lan/mdnshdiscovery.cpp` omits `type`).
- Phone only announces on a trusted network and only discovers for 30 s after start / network change
  (`A:...MdnsDiscovery.kt` `startDiscovering`, `DISCOVERY_TIMEOUT_MILLIS`).
- Phone does **not** read TXT records. On finding a service whose name is not its own id and not already connected, it
  resolves it and sends its UDP identity **unicast to port 1716** of the resolved address (prefers private IPv4)
  (`A:...MdnsDiscovery.kt` `onServiceFound`, `onServiceResolved`, `pickAddress`). So mDNS only helps if we also listen on
  UDP 1716.
- mDNS is optional: the phone always runs the UDP 1716 listener too (`A:...LanLinkProvider.kt` `onStart`,
  `setupUdpListener`).

## 2. TCP + TLS handshake

### 2.1 Roles

The device that **received** the UDP identity opens TCP to the sender's `tcpPort`. Rule on both sides:
**TCP client = TLS server; TCP server = TLS client** (`A:...LanLinkProvider.kt` `identityPacketReceived` comment "If I'm the
TCP server I will be the SSL client"; `K:...lanlinkprovider.cpp` `tcpSocketConnected` -> `startServerEncryption`,
`tcpPacketReceived` -> `startClientEncryption`).

Case A, phone broadcast, we connect (`A:...LanLinkProvider.kt` `tcpPacketReceived`):
1. We TCP-connect to phone's `tcpPort` and send one plaintext identity line. Must pass `isValidIdentityPacket`
   (so include `deviceName`). Optional `targetDeviceId` (must equal phone id) and `targetProtocolVersion` (must equal 8).
2. Phone becomes TLS **client**; we are TLS **server** and must request a client certificate.

Case B, we broadcast, phone connects (`A:...LanLinkProvider.kt` `udpPacketReceived`):
1. Phone TCP-connects to our `tcpPort` and sends its full identity plus `targetDeviceId` and `targetProtocolVersion`.
   `targetProtocolVersion` is copied with `getString`, so it arrives as a **JSON string** (`"8"`).
2. Phone becomes TLS **server** (`useClientMode=false`, `wantClientAuth` if untrusted, `needClientAuth` if trusted)
   (`A:helpers/security/SslHelper.kt` `configureSslSocket`); we are TLS **client** and must send our cert.

Socket timeouts on phone: 10 s read timeout during handshake (`A:...LanLinkProvider.kt` `configureSocket`). Desktop
aborts if TLS + secure identity are not done within 1 s (`K:...lanlinkprovider.cpp` `newTcpConnection`, `encrypted`).

### 2.2 TLS parameters

- Phone: `SSLContext.getInstance("TLSv1.2")` ("TLS up to 1.2, since 1.3 seems to cause issues"), default Android cipher
  suites, no cipher list set (`A:helpers/security/SslHelper.kt` `createSslContext`). Desktop sets no protocol or cipher
  (`K:...lanlinkprovider.cpp` `configureSslSocket`). **Offer TLS 1.2** with ECDHE + ECDSA/RSA AES-GCM suites.
- Untrusted peer: phone uses a trust-all `X509TrustManager` (accepts any cert) (`SslHelper.kt` `trustAllCerts`).
- Trusted (paired) peer: phone trust store contains **only the stored peer cert**; a different cert fails the handshake
  (`SslHelper.kt` `createSslContext` `TrustedDevice`). No hostname/CN check on phone.
- Desktop: trusted peer -> `VerifyPeer` with the stored cert as CA and `setPeerVerifyName(deviceId)`; untrusted ->
  `QueryPeer`; only `SelfSignedCertificate` errors tolerated (`K:...lanlinkprovider.cpp` `configureSslSocket`, `sslErrors`).
  Desktop also aborts unless peer cert CN == identity `deviceId` (`K:...lanlinkprovider.cpp` `addLink`).

### 2.3 Certificate (what we must generate)

- Self-signed X.509, subject = issuer = `CN=<deviceId>, OU=KDE Connect, O=KDE` (`A:SslHelper.kt` `initialiseCertificate`;
  `K:core/sslhelper.cpp` `generateSelfSignedCertificate`).
- Key: EC P-256 (`secp256r1`/`prime256v1`) on both current implementations, signed with SHA-512
  (`SHA512withECDSA`; `EVP_sha512`). RSA keys still accepted (legacy `keyAlgorithm` default RSA, `SHA512withRSA`)
  (`A:helpers/security/RsaHelper.kt` `initialiseRsaKeys`; `K:core/sslhelper.cpp` `generateEcPrivateKey`;
  `K:core/kdeconnectconfig.cpp` `generatePrivateKey`).
- Validity: notBefore = now - 1 year, notAfter = now + 10 years (`SslHelper.kt`; `sslhelper.cpp` `X509_gmtime_adj`).
  Serial: phone uses 1, desktop random 160-bit.
- Phone regenerates its own cert (and **drops all trusted devices**) if CN != its deviceId or cert expired / not yet valid
  (`A:SslHelper.kt` `initialiseCertificate`).

### 2.4 v8 secure identity exchange

Immediately after the TLS handshake, **both sides send their full identity packet over TLS without waiting**, then
read one line (`A:...LanLinkProvider.kt` `identityPacketReceived` handshake listener; `K:...lanlinkprovider.cpp` `encrypted`).
Checks on the received secure identity (both sides): valid identity packet, `protocolVersion` equal to the plaintext
one, `deviceId` equal to the plaintext one; otherwise close. The plaintext identity is not trusted for anything else.
The phone builds `DeviceInfo` (name, type, capabilities) from the **secure** identity + peer cert
(`A:DeviceInfo.kt` `fromIdentityPacketAndCert`). If a link for that id already exists and the cert differs, the new
socket is closed (`A:...LanLinkProvider.kt` `addOrUpdateLink`; `K:...lanlinkprovider.cpp` `addLink`).

## 3. Pairing (`kdeconnect.pair`)

- Request: `{"pair": true, "timestamp": <i64 unix seconds>}`; accept: `{"pair": true}` (no timestamp);
  reject / cancel / unpair: `{"pair": false}` (`A:PairingHandler.kt` `requestPairing`, `acceptPairing`, `cancelPairing`,
  `unpair`; `K:core/backends/pairinghandler.cpp`).
- Timestamp tolerance: **+-1800 s** (30 min) vs. receiver clock; out of range -> "clocks not match" failure; missing
  timestamp on a request -> treated as unpair on phone (`A:PairingHandler.kt` `allowedTimestampDifferenceSeconds`,
  `packetReceived`; `K:...pairinghandler.cpp` `ALLOWED_TIMESTAMP_TIME_DIFFERENCE_SECONDS`).
- Timeouts: requester waits **30 s**; phone shows an incoming request for **25 s** (`A:PairingHandler.kt`); desktop 30 s
  both ways and sends `pair:false` on timeout (`K:core/backends/pairinghandler.h` `pairingTimeoutMsec`, `.cpp` `pairingTimeout`).
- State machine on phone (`A:PairingHandler.kt` `packetReceived`): `pair:true` while `Requested` -> paired;
  while `RequestedByPeer` -> ignored; while `Paired` -> **phone unpairs first**, then treats it as a new request
  (needs timestamp). Desktop instead ignores `pair:true` without timestamp when paired. `pair:false` -> cancel or unpair.
- On paired: phone stores peer cert, name, type, protocolVersion (TOFU) (`A:Device.kt` `pairingSuccessful`;
  `A:DeviceInfo.kt` `saveInSettings`). On unpair: removes trusted device, notifies plugins (`A:Device.kt` `unpaired`).
- **Verification key** (both sides identical; `A:PairingHandler.kt` `getVerificationKey`;
  `K:...pairinghandler.cpp` `verificationKey`):
  1. `a` = own cert public key, `b` = peer cert public key, each as DER **SubjectPublicKeyInfo** (Java
     `publicKey.encoded`, Qt `publicKey().toDer()`).
  2. Order: compare bytewise unsigned; **larger first** (`if a < b: swap`).
  3. `SHA-256(larger || smaller || ascii_decimal(timestamp))`, timestamp = the request's `timestamp` (seconds).
  4. Lowercase hex, first **8** chars, uppercased.
  Only defined while a pairing is in progress on v8.
- Non-pair packet from an unpaired device: phone **calls `unpair()`, which sends `{"pair":false}` back**, and the packet
  reaches no plugin (`A:Device.kt` `onPacketReceived`); desktop does the same (`K:core/device.cpp` `privateReceivedPacket`).
- Trusted device whose cert changed: phone's TLS trust store only holds the old cert, so the handshake fails and the
  device stays unreachable until the user unpairs on the phone (`A:SslHelper.kt` `createSslContext`). Also a live link
  is never replaced by a socket with a different cert (`addOrUpdateLink`).

## 4. Payload transfer

- Sender opens a TCP listener on first free port in **1739..1764**, puts `"payloadSize": n,
  "payloadTransferInfo": {"port": p}` in the packet, then **accepts and acts as TLS server**; receiver connects to
  the main link's peer IP at `port` and is **TLS client** (`A:backends/lan/LanLink.kt` `sendPacket`, `sendPayload`,
  `receivedNetworkPacket`; `A:...LanLinkProvider.kt` `PAYLOAD_TRANSFER_MIN_PORT`; `K:core/backends/lan/compositeuploadjob.cpp`
  `startListening`, `startServerEncryption`; `K:...landevicelink.cpp` `connectToHostEncrypted`).
- Payload TLS uses the **trusted** context (stored peer cert only, client auth required) on both ends
  (`LanLink.kt` `isDeviceTrusted = true`; `K:...landevicelink.cpp` `configureSslSocket(..., true)`), so payloads only work
  after pairing. No identity exchange on payload sockets; raw bytes then close.
- Phone accept timeout 10 s (`LanLink.kt` `sendPayload` `setSoTimeout`).
- `payloadSize: -1` = unknown size: phone sends it when the size is unknown (`A:helpers/FilesHelper.kt`
  `uriToNetworkPacket` `sizeDefault`), and when receiving reads to EOF and skips the size check
  (`A:plugins/share/CompositeReceiveFileJob.kt` `receivePacket`). Desktop documents the same (`K:core/filetransferjob.cpp`).
- Desktop batches several files of one share on a single listener (`K:...compositeuploadjob.cpp`).

## 5. Packet schemas

Direction: P->D = phone sends, D->P = desktop (us) sends. Android files under `A:plugins/`.

- **ping** `kdeconnect.ping` both ways; optional `message: string` shown as notification (`ping/PingPlugin.kt`).
- **battery** `kdeconnect.battery` both ways: `currentCharge: int` (0-100), `isCharging: bool`, `thresholdEvent: int`
  (0 none, 1 battery low) (`battery/BatteryPlugin.kt`). No request packet exists.
- **clipboard** `kdeconnect.clipboard` both ways: `content: string`. `kdeconnect.clipboard.connect` both ways on
  connect: `content`, `timestamp: i64 ms`; phone ignores it if timestamp is 0 or older than its own clipboard
  (`clipboard/ClipboardPlugin.kt` `onPacketReceived`, `sendConnectPacket`).
- **notification** (`notifications/NotificationsPlugin.kt`):
  - P->D `kdeconnect.notification`: `id: string` (Android key), `isClearable: bool`, `appName`, `time: string` (ms),
    `silent: bool`, `title`, `text`, `ticker`, optional `requestReplyId: string` (UUID, repliable), `actions: string[]`
    (titles of non-reply actions), `conversation: [{sender, content}]`, `groupName`, `payloadHash: string`
    (MD5 of PNG icon, uppercase hex). Icon payload is attached only when the hash changed for that `id`. Removal:
    `{id, isCancel: true}`. On plugin start the phone resends all current ones with `silent: true`.
  - D->P `kdeconnect.notification.request`: `{request: true}` = send all; `{cancel: "<id>"}` = dismiss on phone.
  - D->P `kdeconnect.notification.reply`: `requestReplyId`, `message`.
  - D->P `kdeconnect.notification.action`: `key` (= notification `id`), `action` (= action title).
  - D->P `kdeconnect.notification` is handled by `receivenotifications/ReceiveNotificationsPlugin.kt` (disabled by default).
- **sms** (`sms/SMSPlugin.kt`, `helpers/SMSHelper.kt` `Message.toJsonObject`):
  - P->D `kdeconnect.sms.messages`: `version: 2`, `messages: [msg]`, msg = `addresses: [{address}]`, `body`,
    `date: i64 ms`, `type: int` (Android MESSAGE_TYPE_*: 1 inbox, 2 sent...), `read: int`, `thread_id: i64`,
    `_id: i64`, `sub_id: int`, `event: int` (bit 0x1 text, 0x2 multi-target), optional `attachments: [{part_id: i64,
    mime_type, encoded_thumbnail?: base64, unique_identifier}]`.
  - D->P `kdeconnect.sms.request_conversations`: empty body; reply is one `sms.messages` packet **per conversation**
    (latest message each). Also turns on live push of new messages (`haveMessagesBeenRequested`).
  - D->P `kdeconnect.sms.request_conversation`: `threadID: i64`, optional `rangeStartTimestamp: i64 ms`,
    `numberToRequest: i64`; one `sms.messages` reply.
  - D->P `kdeconnect.sms.request`: `version: 2`, `addresses: [{address}]` (fallback legacy `phoneNumber`),
    `messageBody`, optional `subID: i64` (camelCase in code; doc comment says `sub_id`, wrong),
    `attachments: [{fileName, base64EncodedFile, mimeType}]`. Desktop sends the same (`K:plugins/sms/smsplugin.cpp` `sendSms`).
  - D->P `kdeconnect.sms.request_attachment`: `part_id: i64`, `unique_identifier`; P->D
    `kdeconnect.sms.attachment_file`: `filename` (= unique_identifier) + payload (`sms/SmsMmsUtils.kt`).
- **telephony** (`telephony/TelephonyPlugin.kt`): P->D `kdeconnect.telephony`: `event` = `ringing`|`talking`|`missedCall`,
  `phoneNumber`, `contactName`, `phoneThumbnail` (base64), and on hang-up the last packet is resent with
  `isCancel: "true"` (**a string**). D->P `kdeconnect.telephony.request_mute`: empty, silences ringer.
- **share** (`share/SharePlugin.kt`, `helpers/FilesHelper.kt`, `share/CompositeUploadFileJob.kt`):
  `kdeconnect.share.request` both ways; exactly one of: `filename` + payload (+ `lastModified: i64 ms`, `open: bool`,
  `numberOfFiles: int`, `totalPayloadSize: i64`), `text` (phone copies to clipboard), `url` (phone opens it).
  `kdeconnect.share.request.update`: `numberOfFiles`, `totalPayloadSize` to grow a running batch. Packets without
  `numberOfFiles` or with `open: true` are not batched.
- **findmyphone** D->P `kdeconnect.findmyphone.request`, empty body; phone rings (`findmyphone/FindMyPhonePlugin.kt`).
  The phone sends the same type to make the desktop ring (`findremotedevice/FindRemoteDevicePlugin.kt`).
- **contacts** (`contacts/ContactsPlugin.kt`): D->P `kdeconnect.contacts.request_all_uids_timestamps` (empty) -> P->D
  `kdeconnect.contacts.response_uids_timestamps`: `uids: string[]` plus one key per uid -> `i64` timestamp.
  D->P `kdeconnect.contacts.request_vcards_by_uid`: `uids: string[]` -> P->D `kdeconnect.contacts.response_vcards`:
  `uids` plus uid -> vCard string (adds `X-KDECONNECT-ID-DEV-<ourId>` and `REV`). Needs a per-device user consent on the
  phone (`checkRequiredPermissions` `acceptedToTransferContacts`).
- **connectivity_report** P->D `kdeconnect.connectivity_report`: `signalStrengths: {"<subId>": {networkType: string,
  signalStrength: int 0-4}}` (`connectivityreport/ConnectivityReportPlugin.kt`). **No request packet exists** in either
  repo (`K:plugins/connectivity-report/*.json` declares no outgoing type).
- **mpris** (brief, `mpris/MprisPlugin.kt`): P->D `kdeconnect.mpris.request` `{requestPlayerList:true}`,
  `{player, requestNowPlaying:true, requestVolume:true}`, `{player, action: Play|Pause|PlayPause|Next|Previous|Stop}`,
  `{player, Seek|SetPosition|setVolume|setLoopStatus|setShuffle: value}`; D->P `kdeconnect.mpris`: `playerList`,
  `supportAlbumArtPayload`, per player `title`, `artist`, `album`, `isPlaying`, `pos`, `length`, `volume`, `can*`,
  `albumArtUrl`; album art as payload with `transferringAlbumArt: true`. Reverse direction in `mprisreceiver/`.
- **mprisreceiver** (phone's media for us to control; `plugins/mprisreceiver/MprisReceiverPlugin.kt`, desktop
  `K:plugins/mprisremote/`): P->D `kdeconnect.mpris` `{playerList:[..], supportAlbumArtPayload:true}` on session
  changes; per-player `{player, title, artist, album, nowPlaying, isPlaying, canPlay, canPause, canGoNext,
  canGoPrevious, canSeek, pos (ms), length (ms), volume (0-100), albumArtUrl (key, "" if none)}` on every playback
  callback (no periodic position: extrapolate). D->P `kdeconnect.mpris.request`: `{requestPlayerList:true}`,
  `{player, requestNowPlaying:true}`, `{player, action: Play|Pause|PlayPause|Next|Previous|Stop}`,
  `{player, setVolume}`, `{player, SetPosition (ms, absolute)}`; relative `Seek` is ignored by the phone.
  Art: `{player, albumArtUrl}` alone (the phone returns early, so no other field) -> `kdeconnect.mpris`
  `{player, transferringAlbumArt:true, albumArtUrl}` + JPEG payload, only if the key is still current.
  Capabilities: our incoming `kdeconnect.mpris`, outgoing `kdeconnect.mpris.request`.
- **sftp** (`sftp/SftpPlugin.kt`, `sftp/SimpleSftpServer.kt`): D->P `kdeconnect.sftp.request` `{startBrowsing: true}`.
  P->D `kdeconnect.sftp`: `ip`, `port` (1739..1764), `user: "kdeconnect"`, `password` (fresh each time), `path`
  (single root or `/`), `multiPaths: string[]`, `pathNames: string[]`; or `{errorMessage}`; `{serverRunning: false}` sent on
  plugin create. Server auth: password **or** public key equal to our TLS cert's public key (`SimplePublicKeyAuthenticator`).
- **mousepad** (brief): `kdeconnect.mousepad.request` fields `dx`, `dy` (double), `singleclick`, `doubleclick`,
  `middleclick`, `rightclick`, `singlehold`, `singlerelease`, `scroll`, `key`, `specialKey: int`, `alt`/`ctrl`/`shift`/`super`
  (`mousepad/MousePadPlugin.kt`, `mousereceiver/MouseReceiverPlugin.kt`); D->P `kdeconnect.mousepad.keyboardstate`
  `{state: bool}`; `kdeconnect.mousepad.echo` from the remote keyboard IME.
- **runcommand** (brief, `runcommand/RunCommandPlugin.kt`): D->P `kdeconnect.runcommand`: `commandList` = **a JSON
  string** encoding `{key: {name, command}}`, `canAddCommand: bool`; P->D `kdeconnect.runcommand.request`: `{key}`,
  `{requestCommandList: true}`, `{setup: true}`, `{key, stop: true}`; D->P `kdeconnect.runcommand.output`:
  `id`, `commandStarted`/`commandOutput` (`stdout`, `stderr` string[])/`commandFinished` (`success`).
- **Call control / screen sharing**: none on Android. Telephony only reports and mutes; no answer/hang-up packet exists
  (`telephony/TelephonyPlugin.kt` `supportedPacketTypes`). `kdeconnect.virtualmonitor` exists only on desktop
  (`K:plugins/virtualmonitor`), no Android plugin. Other Android types: `kdeconnect.systemvolume(.request)`,
  `kdeconnect.presenter`, `kdeconnect.digitizer(.session)`, `kdeconnect.shareinputdevices(.request)`.

## 6. How the phone decides which plugins run

- Until a non-empty capability set is received, all plugins are assumed supported (`A:Device.kt` constructors,
  `supportedPlugins = PluginFactory.availablePlugins`). Capabilities are only adopted when **both** lists are non-empty
  (`A:Device.kt` `updateDeviceInfo`).
- A phone plugin is loaded iff our `outgoingCapabilities` intersects its `supportedPacketTypes`, **or** our
  `incomingCapabilities` intersects its `outgoingPacketTypes` (`A:plugins/PluginFactory.kt` `pluginsForCapabilities`).
- Then: device paired and reachable, plugin enabled (per-device setting, default `isEnabledByDefault`), Android
  permissions granted, `minSdk` met (`A:Device.kt` `reloadPluginsFromSettings`, `isPluginEnabled`). Disabled by default:
  receivenotifications; digitizer except on tablets.
- The phone **refuses to send** any packet type not in our `incomingCapabilities` (once known)
  (`A:Device.kt` `sendPacketBlocking` `supportsPacketType`).
- Desktop routes incoming packets by plugin `X-KdeConnect-SupportedPacketType` (`K:plugins/*/*.json`). To get each phone
  feature, advertise:

  | feature | our incoming | our outgoing |
  | --- | --- | --- |
  | ping | `kdeconnect.ping` | `kdeconnect.ping` |
  | battery | `kdeconnect.battery` | `kdeconnect.battery` |
  | clipboard | `kdeconnect.clipboard`, `kdeconnect.clipboard.connect` | same |
  | notifications | `kdeconnect.notification` | `kdeconnect.notification.request`, `.reply`, `.action` |
  | sms | `kdeconnect.sms.messages`, `kdeconnect.sms.attachment_file` | `kdeconnect.sms.request`, `.request_conversations`, `.request_conversation`, `.request_attachment` |
  | telephony | `kdeconnect.telephony` | `kdeconnect.telephony.request_mute` |
  | share | `kdeconnect.share.request` | `kdeconnect.share.request`, `kdeconnect.share.request.update` |
  | find my phone | `kdeconnect.findmyphone.request` (phone rings us) | `kdeconnect.findmyphone.request` |
  | contacts | `kdeconnect.contacts.response_uids_timestamps`, `.response_vcards` | `kdeconnect.contacts.request_all_uids_timestamps`, `.request_vcards_by_uid` |
  | connectivity | `kdeconnect.connectivity_report` | - |
  | sftp | `kdeconnect.sftp` | `kdeconnect.sftp.request` |
  | we control the phone's media | `kdeconnect.mpris` | `kdeconnect.mpris.request` |
  | phone controls our media | `kdeconnect.mpris.request` | `kdeconnect.mpris` |
  | phone as touchpad | `kdeconnect.mousepad.request` | `kdeconnect.mousepad.keyboardstate` |
  | run commands | `kdeconnect.runcommand.request` | `kdeconnect.runcommand`, `kdeconnect.runcommand.output` |

## 7. Traps

1. TLS roles are inverted vs. TCP roles (TCP client = TLS server). Both TLS server sides must **request** a client
   cert, or the peer cert is unknown and pairing/TOFU break.
2. Phone's TLS context is TLS 1.2 only. A TLS-1.3-only server/client will not connect.
3. Phone rate limit shares one per-IP map between UDP and TCP (1 s). Sending our UDP broadcast and opening a TCP
   connection to the phone within 1 s from the same IP gets the TCP dropped.
4. Every identity, including the minimal UDP one and the plaintext TCP one, needs a non-blank `deviceName` and a regex-valid
   `deviceId`, or the phone silently ignores it.
5. `targetProtocolVersion` from the phone is a string; parse ints leniently.
6. A packet with `payloadSize` but no `payloadTransferInfo`, or missing `body`, makes the phone's parser throw, which
   ends the read loop and **drops the whole link** (`A:backends/lan/LanLink.kt` `startListening` catch-all).
7. Never send non-pair packets before pairing completes: the phone answers with `pair:false` and fires its unpaired path.
8. Never send `pair:true` to a phone that thinks it is paired unless re-pairing: the phone unpairs first.
9. Pair request timestamp is **seconds**, clipboard.connect timestamp is **milliseconds**, SMS `date` is milliseconds,
   notification `time` is a **string** of milliseconds.
10. Keep our cert stable. A new cert (or CN != deviceId) locks us out of an already paired phone until the user unpairs.
11. Phone sends nothing to types missing from our `incomingCapabilities`, and loads plugins only from capability
    overlap, so a typo in a capability silently disables a feature.
12. Payload sockets use the trusted TLS config: before pairing, payloads (icons, files) cannot be fetched.
13. `telephony` `isCancel` is the string `"true"`; notification `isCancel` is a boolean.
14. Phone only accepts LAN peers from private / CGNAT / ULA addresses; a public IPv6 source is dropped.

## Not verified here

- Exact cipher suites negotiated (Android defaults vary by OS version; only the protocol is pinned).
- Whether Android's PKIX path check enforces notBefore/notAfter on the stored self-signed peer cert.
- Bluetooth backend (`A:backends/bluetooth/`) was not reviewed.
