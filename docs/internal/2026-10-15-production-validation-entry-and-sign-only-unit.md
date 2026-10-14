# 原App专用持久凭据验证入口及私有签包工具

本单元是开发源码/配置/资格控制与类型检查。global、normal07、sealed13和旧A/B/S没有修改或重标；没有真实SecItem、签新包、Core/provider运行、网络连接、安装或设备动作。实际生产签名/持久恢复仍未完成。

## 一个明确编译通道，先资格后启动

原PendingCrew target新增Release型DeliveryValidation配置及同名专用scheme，全actions明确使用该配置。DeliveryValidation uses the existing allowed native-candidate bundleID, with independent validation URL/Info marker，PENDINGCREW_DELIVERY_VALIDATION条件、默认不签、无Team/profile/entitlements、不读个人Local配置。正式Debug/Release与NativeCandidate的全部既有配置对象保持不变。工程只新增CrewdDeliveryValidation.swift这个实际source-phase文件，原source全部保留；第二次xcodegen无漂移。

stamp在签名前要求PENDINGCREW_VALIDATION_SOURCE_REF为40个lowerhex，与产品Info的sourceRef/validation marker绑定。缺坏值失败且原Info不变。运行时入口在所有旧MCP/daemon/helper与GUI分支前核实际bundle marker/ref和私有manifest，validation只允许无附加argv的原GUI入口；坏资格直接诊断退出，Core/legacy/Keychain不启动。SourceRef是来源关联，不是用户/服务器身份授权。

真实CrewdRemoteCredentials.application()的validation分支只返回资格固定的CrewdValidationCredentials，或返回全方法拒绝的store；不会fallbackproduction/memory。普通factory仍正式非同步Keychain class/稳定service，原显式memory fixture和candidate恒memory不变。测试本机record/account来自预列唯一UUID，普通/candidate Pairing仍随机UUID。这里不预造任何server grant/realm/runtime/native ID。

## Mac与移动是两个不同私有作用域

共同清单是closed字段、format1、固定fake-only验证域、完整源码ref、scope UUID、唯一service=com.pendingname.pendingcrew.delivery-validation.<scopeUUID>、唯一account及逐操作records/read/save/remove许可。所有查询只这个service/account，不使用共享组、不枚举未列记录。records逐个精确account查询，不调用service-wide枚举；原普通store方法不变。返回store固定最初manifest SHA和root/service/account，后续每个动作重新资格核对，修改/切换清单会在SecItem前拒绝。

Mac只接受/private/tmp/pcw-delivery-validation.<UUID>/scope.json，现存owned0700、文件0600、canonical且无alias；data/work/bin必须同root下owned0700。data env must match; every PENDINGCREW_CREWD_PROGRAM override is refused; fakeCodex只能该bin/fake-codex、owned0700 regular且匹配清单hash，Claude只能明确/usr/bin/false及其hash。读取scope用O_RDONLY|NOFOLLOW|CLOEXEC并复核descriptor type/UID/mode/size/mtime。所有资格检查不创建、chmod或采纳不可信目录。

移动是remote-only：实际container由NSHomeDirectory提供，不从env替换；scope只允许其Library/Application Support/PendingCrewDeliveryValidation/<UUID>私有节点，presentation同scope下owned0700。Mac provider/data env全部拒绝，没有local Core或服务器路径猜测。当前控制中mobile container是明确的private modeled container，不是真机OS容器运行证明。

两个作用域都明确预列endpoint及expected真实server authority/database/SPKI。清单固定fake-only域字符串/hostname本身**不授予信任**，source控制中的127.0.0.1只是未连接的结构fixture，不是可达手机后台或已审网络。未来action manifest需由受审fake后台实际输出取得这些值、列具体可达endpoint/network及批准操作，不能猜realm/pin/grant或创建公共测试服务。

Pairing先完成既有正式签名邀请/certificate/SPKI/有效期验证，再核上述预列范围；原签名回应/确认码/receipt/grant/TLS资格不变。validation store read/save也调用真实邀请验签及原scope范围，不能写其它authority的材料；读后再次核原scope。真正的grant与权限只由正式后台返回及现协议校验，清单不是旁路。

## 纯私有sign-only入口

新build-crewd-production-sign-only.py默认只plan，不执行Security/Xcode/codesign。closed manifest必须给明确40hex sourceRef、真实正常helper compiledRef/profile、Team、证书SHA1、profile UUID及private-copy hash、原warm DD/SPM；没有默认Team/identity，不调用旧release脚本的notary/feed/upload。默认plan明确不证明profile/私钥可用性。

后续--execute需要单独核人类签包授权。工具从完整committed archive取来源，recipe必须匹配该ref；正常helper需完整fresh-own输入/config/raw证据，并在GUI构建前复制/核对唯一presign字节。实际profile decode/UUID/Team/expiry/applicationID/no-debug先验，手动签名输入只写私有snapshot Local；Release/prodID/no candidate或validation宏/Hardened YES、原warm缓存实际settings先验后archive，不允许自动provisioning。archive必须嵌入逐字reviewed profile；嵌helper后只指定证书签helper/App，实际DeveloperID链/Team/hardened/no-debug/deepstrict验证，保存完整Git/显式Local override/正常helper来源/SPM前后/实际products。sign-only没有公证、上传、安装或App启动。此工具执行分支尚未真实签包，不能称已验证Xcode/证书/profile兼容。

## 实际证据与失败归因

证据在/private/tmp/pcw-delivery-validation-source-unit-01，最终whole binding关联本次源hash。

- 三平台真实DeliveryValidation showBuildSettings确认新channel/ID/宏、不签/noTeam/entitlements；原九Debug/Release/NativeCandidate配置对象逐值一致，source phase仅一项新增。
- 实际production stamp五控制，缺坏ref拒绝且原plist bytes不变。
- factory-controls-final实际application()选择受限persistent store并执行未列account/action、个人service、坏ref、wide/alias、manifest变化与移动scope负例；普通/candidate实际factory class选择控制也通过。实际Foundation/文件资格代码运行，零SecItem/Core/provider；移动container模型与纯邀请范围方法不冒移动App、签名邀请正例或真实服务器信任。
- no-SecItem控制使用一个永不应调用的Pairing bridge供链接；实际认证Pairing源在完整typecheck中链接。没有用这个bridge证明验签、读写或连接。两进程真实Keychain保存/重读/撤销/删除仍待精确fake-service/action授权与实际完整App运行。
- 初版95764 driver -5是资格代码采用Foundation standardized/resolving路径时，系统把/private/tmp归一为/tmp，触严格拒绝。修正发生在**生产资格代码**，改为POSIX realpath等于原absolute字符串；/private/tmp正例可用，任意/tmp/system或scope/data alias仍拒。没有通过放宽alias或chmod换绿；原diagnostic日志留存。
- 83847默认sandbox完整Mac类型检查的首个错误是SwiftUI宏服务malformed response，后续State不可变诊断是级联；原日志保留，不先认产品ABI。最终77431在明确CLI编译授权上下文沿原warm缓存完整三平台370个源、正确SDK/arm64 target/validation宏且无DEBUG/candidate均exit0。它不是全link、签包或设备运行。

下一整合由根把此单元与后端accepted恢复cut按完整字节组合，生成唯一准确normal与完整包；不拿13重标新源、不打各小项中间包。实际Keychain、publisher、新签候选/真机、安装/日常数据与发布分别准备准确action manifest再核授权。旧installed0.1.43严格签名失败与目前设备不可用事实原样保留。

## Whole review closure and actual execution dependencies

The original source-review cut 9bf68cad and its 370-source checks retain their actual input identities and rejection report. The same-unit closure fixes the saved/query lowercase account versus uppercase returned-attribute comparison through one production canonicalAccount helper used by both query and boundedRecords. Controlled API attributes test the actual production matcher without SecItem; it does not expand enumeration. The manifest now rejects fractional/boolean format types, rather than trusting NSNumber.intValue truncation.

DeliveryValidation now uses the existing allowed native-candidate identifier. Its independent Release-type compile channel, validation marker, private fake service and scope remain; ordinary and original NativeCandidate config values remain unchanged. It does not expand the backend code allowlist or qualify as publisher. Mac and mobile scopes reject every PENDINGCREW_CREWD_PROGRAM override, including empty strings. A later launch manifest must bind the accurately sealed embedded normal helper/source/hash; the scope itself cannot authorize a process.

The separate ordinary prodID publisher test uses the existing explicit PENDINGCREW_ISOLATED_CREDENTIALS=memory application() path, which returns the isolated singleton before constructing the personal production Keychain. The future private launch action must declare this exact value. This demonstrates OS publisher/newPID permissions, never persistent recovery. Actual fake-persistent validation is through the native-candidate validation App processes and formal pairing/TLS, not a prod-service access or fabricated identity.

The signing tool now evaluates a real OS test requirement: anchor apple generic, production identifier, approved leaf OU Team, DeveloperID intermediate OID1.2.840.113635.100.6.2.6 and leafOID1.2.840.113635.100.6.1.13. This is separate from strict deep code sealing and hardened/no-debug checks. Authority names alone do not prove an Apple chain. The requirement log enters static product proof; actual new-PID dynamic SecCode admission remains a later gate. Source expression controls prove the tool sends the hard requirement, not actual certificate/OS positivity.

Closure evidence is under closure-controls, closure-settings, closure-typechecks and the accurate new-ref sign-only plan controls. The prior full 370 App checks remain 9bf evidence; the bounded three-SDK closure checks include the changed production sources and explicit type-only Pairing bridge, not a new full App link. None of this invokes Keychain, signs, installs, runs an App/daemon/provider or connects to an endpoint. The final combined package still needs all accurate inputs/config/source and actual runtime scopes.
