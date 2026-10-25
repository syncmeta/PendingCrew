import Foundation

@main struct AppleServiceQualificationRegression {
    static func main() {
        let bundle="com.pendingname.pendingcrew"
        let run="11111111-2222-4333-8444-555555555555"
        let key="PENDINGCREW_APPLE_QUALIFICATION_RUN_ID"
        let environment=[key:run]
        let expectCloud=CommandLine.arguments.contains("--expect-cloud")
        let expectAPNs=CommandLine.arguments.contains("--expect-apns")
        let normal=CrewAppleServiceDevelopment.cloudConfiguration(environment:[:],bundleIdentifier:bundle)
        precondition((normal != nil)==expectCloud,"Normal opening must not require a qualification launch environment")
        precondition(CrewAppleServiceDevelopment.apnsSandboxEnabled(environment:[:],bundleIdentifier:bundle)==expectAPNs,"Normal Sandbox opening must not require a qualification launch environment")
        if let normal {
            precondition(normal.containerIdentifier=="iCloud.com.pendingname.pendingcrew")
            precondition(normal.environment == .development)
            precondition(normal.draftsCollection=="drafts" && normal.hostsCollection=="hosts")
            precondition(normal.keyValueStoreIdentifier==nil)
        }
        let cloud=CrewAppleServiceDevelopment.cloudConfiguration(environment:environment,bundleIdentifier:bundle)
        precondition((cloud != nil)==expectCloud)
        precondition(CrewAppleServiceDevelopment.apnsSandboxEnabled(environment:environment,bundleIdentifier:bundle)==expectAPNs)
        precondition(CrewAppleServiceDevelopment.qualificationRunID(environment:environment,bundleIdentifier:bundle)==run)
        for invalid in [[key:""],[key:"not-a-uuid"],[key:"AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE"]] {
            precondition(CrewAppleServiceDevelopment.qualificationRunID(environment:invalid,bundleIdentifier:bundle)==nil)
            precondition(CrewAppleServiceDevelopment.cloudConfiguration(environment:invalid,bundleIdentifier:bundle)==nil)
            precondition(!CrewAppleServiceDevelopment.apnsSandboxEnabled(environment:invalid,bundleIdentifier:bundle))
        }
        for foreign in [nil,"com.pendingname.pendingcrew.hotreload","com.pendingname.pendingcrew.native-candidate"] {
            precondition(CrewAppleServiceDevelopment.cloudConfiguration(environment:[:],bundleIdentifier:foreign)==nil)
            precondition(!CrewAppleServiceDevelopment.apnsSandboxEnabled(environment:[:],bundleIdentifier:foreign))
            precondition(CrewAppleServiceDevelopment.qualificationRunID(environment:environment,bundleIdentifier:foreign)==nil)
            precondition(CrewAppleServiceDevelopment.cloudConfiguration(environment:environment,bundleIdentifier:foreign)==nil)
            precondition(!CrewAppleServiceDevelopment.apnsSandboxEnabled(environment:environment,bundleIdentifier:foreign))
        }
        if let cloud {
            precondition(cloud.containerIdentifier=="iCloud.com.pendingname.pendingcrew")
            precondition(cloud.environment == .development)
            precondition(cloud.draftsCollection=="qualification.\(run).drafts")
            precondition(cloud.hostsCollection=="qualification.\(run).hosts")
            precondition(cloud.draftsCollection != cloud.hostsCollection)
            precondition(cloud.keyValueStoreIdentifier==nil)
            precondition(cloud.draftsCollection != normal?.draftsCollection)
            precondition(cloud.hostsCollection != normal?.hostsCollection)
            let other=CrewAppleServiceDevelopment.cloudConfiguration(environment:[key:"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"],bundleIdentifier:bundle)!
            precondition(other.draftsCollection != cloud.draftsCollection)
            precondition(other.hostsCollection != cloud.hostsCollection)
        }
        print("PASS explicit cloud=\(expectCloud) apns=\(expectAPNs); normal opening configured; invalid/foreign qualification refused; no SDK/account/network action")
    }
}
