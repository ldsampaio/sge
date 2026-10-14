# Sieve export — rules only (golden fixture: sieve_rules_only.sieve)
# Generated offline (Phase 31 SIEVE-01)
require "fileinto";
# NOTE: regex test is NOT required — output runs on minimal servers.
# NOTE: body :contains needs server body capability; else comment+count.

# Rule: Boletos (id=boletos-2) priority=100
if address :contains "From" "utfpr.edu.br"
    fileinto "Auto/Arquivo";

# End of Sieve export
