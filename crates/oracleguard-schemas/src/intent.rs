//! Disbursement intent types.
//!
//! Owns: canonical `DisbursementIntentV1` (and future versioned intent
//! structs), including field layout, ordering, and canonical byte
//! encoding.
//!
//! Does NOT own: intent construction semantics performed by the shell,
//! transport of the intent, or decision logic over the intent. This
//! module defines *what an intent is*, not *what to do with it*.
