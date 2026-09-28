// dct 的两把钥匙放在 Mac 的安全芯片里（设计：docs/superpowers/specs/2026-09-27-dct-brain-design.md 第 2 节）。
// 私钥永远不出安全芯片；我们拿到的 dataRepresentation 是它加密过的「把手」，只有这台 Mac 能用。
// 状态码：0 成功，1 没有安全芯片，2 把手打不开，3 用户没通过指纹 / 取消，4 缓冲区太小，
// 6 安全芯片的其它错误。`statusOut` 在返回 2 或 6 时带着 `(error as NSError).code`
//（比如 -25308 = errSecInteractionNotAllowed：设备锁屏，或者不在普通终端会话里），
// 别的返回码上是 0。
import CryptoKit
import Foundation
import LocalAuthentication
import Security

@_cdecl("dct_se_available")
public func dct_se_available() -> Bool {
    SecureEnclave.isAvailable
}

/// LAError 的取消码（用户主动取消 / 系统打断 / 应用被切走）映射到 3；
/// 别的一律是 6，`statusOut` 里带上底层的 NSError 代码，好让调用方判断具体原因。
private func classify(_ error: Error, _ statusOut: UnsafeMutablePointer<Int32>) -> Int32 {
    let ns = error as NSError
    statusOut.pointee = Int32(ns.code)
    if ns.domain == LAError.errorDomain,
        let code = LAError.Code(rawValue: ns.code),
        code == .userCancel || code == .systemCancel || code == .appCancel
    {
        return 3
    }
    return 6
}

@_cdecl("dct_se_create")
public func dct_se_create(
    _ biometric: Bool,
    _ blobOut: UnsafeMutablePointer<UInt8>, _ blobCap: Int, _ blobLen: UnsafeMutablePointer<Int>,
    _ pubOut: UnsafeMutablePointer<UInt8>,
    _ statusOut: UnsafeMutablePointer<Int32>
) -> Int32 {
    statusOut.pointee = 0
    guard SecureEnclave.isAvailable else { return 1 }
    var flags: SecAccessControlCreateFlags = [.privateKeyUsage]
    if biometric { flags.insert(.biometryCurrentSet) }
    var err: Unmanaged<CFError>?
    guard let ac = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, &err) else {
        if let err = err {
            statusOut.pointee = Int32(CFErrorGetCode(err.takeRetainedValue()))
        }
        return 6
    }
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: ac)
        let blob = key.dataRepresentation
        guard blob.count <= blobCap else { return 4 }
        blob.copyBytes(to: blobOut, count: blob.count)
        blobLen.pointee = blob.count
        let pub = key.publicKey.x963Representation  // 65 字节，0x04 开头
        guard pub.count == 65 else { return 6 }  // 不该发生；没有具体 NSError 可报，status 留 0
        pub.copyBytes(to: pubOut, count: 65)
        return 0
    } catch {
        return classify(error, statusOut)
    }
}

@_cdecl("dct_se_sign")
public func dct_se_sign(
    _ blob: UnsafePointer<UInt8>, _ blobLen: Int,
    _ msg: UnsafePointer<UInt8>, _ msgLen: Int,
    _ reason: UnsafePointer<CChar>,
    _ sigOut: UnsafeMutablePointer<UInt8>,
    _ statusOut: UnsafeMutablePointer<Int32>
) -> Int32 {
    statusOut.pointee = 0
    guard SecureEnclave.isAvailable else { return 1 }
    let ctx = LAContext()
    ctx.localizedReason = String(cString: reason)  // Touch ID 弹窗里显示的那句人话
    let key: SecureEnclave.P256.Signing.PrivateKey
    do {
        key = try SecureEnclave.P256.Signing.PrivateKey(
            dataRepresentation: Data(bytes: blob, count: blobLen), authenticationContext: ctx)
    } catch {
        statusOut.pointee = Int32((error as NSError).code)
        return 2
    }
    do {
        let sig = try key.signature(for: Data(bytes: msg, count: msgLen))
        sig.rawRepresentation.copyBytes(to: sigOut, count: 64)  // r||s
        return 0
    } catch {
        return classify(error, statusOut)
    }
}
