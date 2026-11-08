import CoreGraphics

let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
for window in list {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    if owner.lowercased() == "workbook2" {
        if let number = window[kCGWindowNumber as String] as? Int {
            print(number)
            break
        }
    }
}
