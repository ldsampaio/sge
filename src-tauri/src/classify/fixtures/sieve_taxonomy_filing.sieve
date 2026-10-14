# Sieve export — taxonomy filing (golden fixture: sieve_taxonomy_filing.sieve)
require "fileinto";
# NOTE: regex test is NOT required.
# NOTE: body :contains needs server body capability.

# Rule: Academico (id=academico) priority=50 → category filing
if header :contains "Subject" "reunião"
    fileinto "Auto/Acadêmico/Reunião";

# End of Sieve export
