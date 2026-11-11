# ADR-212: Message times in the transcript

## Decision

Every sent message shows its time ("HH:mm", or "dd/MM HH:mm" in Portuguese and "MM/dd HH:mm" in
English when it was not sent today) small and muted in the row under its bubble, left of the
copy and edit icons. Unlike the icons, which appear on hover, the time is always visible. A
reply shows its time only on the latest reply of the transcript, after its icons, once the turn
has finished (not while it streams or the conversation works). It uses the reply's `endedAt`
when the activity has one, else the message's `createdAt`. Times are in the machine's local zone
(`native-spike/src/message_time.rs`; `Turn::sent_ms` and `Turn::replied_ms`).
