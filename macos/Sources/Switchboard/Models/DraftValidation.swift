struct DraftValidation: Decodable, Equatable {
    let errors: [FieldIssue]
    let warnings: [FieldIssue]

    static let empty = DraftValidation(errors: [], warnings: [])

    var isValid: Bool { errors.isEmpty }
}
