//! Job identities and compatibility construction.

// Identifies one job across registry lifetimes. Conversion from a string
// performs no validation, matching the source `JobId(id)` function.
hoh_brand::branded_string!(pub JobId);
