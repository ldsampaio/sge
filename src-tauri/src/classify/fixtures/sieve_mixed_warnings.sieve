# Sieve export — mixed warnings (golden fixture: sieve_mixed_warnings.sieve)
require "fileinto";
# NOTE: regex test is NOT required.
# NOTE: body :contains needs server body capability.

# Rule: Regex (id=r-re) priority=50
# WARNING: Regex — subject_regex "^[0-9]+$" has no RFC 5228 regex test (deliberately NOT required)
if address :contains "From" "x"
    fileinto "Arquivo";

# Rule: Body (id=r-b) priority=10
# WARNING: Body — scope.folders (["Sent"]) has no Sieve equivalent
if body :contains "boleto"
    fileinto "Auto/Arquivo";

# End of Sieve export
