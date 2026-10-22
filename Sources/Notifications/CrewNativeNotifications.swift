import Foundation
import Combine
import UserNotifications
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

/// Filled only by a later qualified signed product; this is not an entitlement bypass.
struct CrewNotificationQualification:Equatable {
    let environment:CrewdNotificationWire.Environment
    let topic:String
}
@MainActor final class CrewNativeNotifications:NSObject,ObservableObject,UNUserNotificationCenterDelegate {
    static let shared=CrewNativeNotifications()
    static let qualification:CrewNotificationQualification?=nil
    @Published private(set) var notice:String?
    @Published private(set) var hint:CrewdNotificationWire.Hint?
    @Published private(set) var pending=false
    private var operation:UUID?
    private var received:((Data)async->Void)?
    private var receiving:UUID?
    private var queuedToken:Data?
    private var lastToken:Data?
    private override init(){super.init()}
    static func prepareForApp(){guard qualification != nil else{return};UNUserNotificationCenter.current().delegate=shared}
    func enable(store:CrewdNotificationStore,crews:[String],kinds:[CrewdNotificationWire.Kind],ask:Bool)async {
        guard let qualification=Self.qualification,!pending else{notice="此版本尚未启用系统通知；不会申请权限或注册。";return}
        let id=UUID()
        do{
            let origin=try store.preparePermission()
            guard store.status?.topic==qualification.topic,store.status?.environment==qualification.environment else{throw CrewdNotificationWire.invalid()}
            operation=id;pending=true;lastToken=nil;queuedToken=nil
            let center=UNUserNotificationCenter.current();center.delegate=self
            let allowed:Bool
            if ask{allowed=try await center.requestAuthorization(options:[.alert,.sound,.badge])}
            else{let settings=await center.notificationSettings();allowed=settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional}
            guard operation==id else{return}
            guard allowed else{pending=false;operation=nil;notice="系统尚未允许通知，可在系统设置中查看；不会自动再次询问。";return}
            try store.consent(true,for:origin);try store.saveSubscription(crews:crews,kinds:kinds,for:origin)
            received={ [weak self,weak store] token in
                guard let self,self.operation==id,let store else{return}
                do{try await store.register(token:token,environment:qualification.environment,topic:qualification.topic,crews:crews,kinds:kinds,expectedOrigin:origin);if self.operation==id{self.notice="原设备通知注册已确认。"}}
                catch{if self.operation==id{self.notice="设备通知尚未确认；请在原电脑连接中查询原请求，不会重新注册。"}}
            }
            #if os(macOS)
            NSApplication.shared.registerForRemoteNotifications()
            #elseif os(iOS)
            UIApplication.shared.registerForRemoteNotifications()
            #endif
        }catch{guard operation==id || operation==nil else{return};pending=false;operation=nil;received=nil;notice="当前电脑、通知配置或原请求尚未核对；没有注册其它电脑。"}
    }
    func registered(_ token:Data){
        guard !token.isEmpty,token != lastToken,let id=operation,let received else{return}
        queuedToken=token
        guard receiving != id else{return}
        receiving=id;pending=true
        Task{
            while self.operation==id,let next=self.queuedToken{
                self.queuedToken=nil
                guard next != self.lastToken else{continue}
                self.lastToken=next
                await received(next)
            }
            if self.receiving==id{self.receiving=nil}
            if self.operation==id{self.pending=false}
        }
    }
    func failed(){cancelRegistration();notice="系统设备注册未完成；未向后台提交新设备。"}
    func cancelRegistration(){received=nil;pending=false;operation=nil;queuedToken=nil;lastToken=nil;receiving=nil}
    func consume(_ value:CrewdNotificationWire.Hint){if hint==value{hint=nil}}
    nonisolated func userNotificationCenter(_ center:UNUserNotificationCenter,didReceive response:UNNotificationResponse,withCompletionHandler completionHandler:@escaping ()->Void){
        // Bound Apple payload, decode only its exact pendingCrew envelope.
        let info=response.notification.request.content.userInfo
        let bytes=(try? JSONSerialization.data(withJSONObject:info["pendingCrew"] ?? NSNull()))
        let defaultAction=response.actionIdentifier==UNNotificationDefaultActionIdentifier
        Task{@MainActor in
            if defaultAction,let bytes,bytes.count<=4096,let value=try? JSONDecoder().decode(CrewdNotificationWire.Hint.self,from:bytes){self.hint=value}
            else{self.notice="通知原上下文不可用；未执行操作。"}
        }
        completionHandler()
    }
    nonisolated func userNotificationCenter(_ center:UNUserNotificationCenter,willPresent notification:UNNotification,withCompletionHandler completionHandler:@escaping (UNNotificationPresentationOptions)->Void){completionHandler([.banner,.list])}
}
#if os(iOS)
final class PendingCrewMobileNotificationDelegate:NSObject,UIApplicationDelegate {
    func application(_ application:UIApplication,didFinishLaunchingWithOptions launchOptions:[UIApplication.LaunchOptionsKey:Any]?=nil)->Bool{CrewNativeNotifications.prepareForApp();return true}
    func application(_ application:UIApplication,didRegisterForRemoteNotificationsWithDeviceToken deviceToken:Data){CrewNativeNotifications.shared.registered(deviceToken)}
    func application(_ application:UIApplication,didFailToRegisterForRemoteNotificationsWithError error:Error){CrewNativeNotifications.shared.failed()}
}
#endif
