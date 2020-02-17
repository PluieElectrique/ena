---- MODULE Anchor ----

(* A formal specification of the anchor thread heuristic. We can prove the
   invariant NoFalseDeletions, but not the temporal formula AllDeletions. That
   is, we prove that the anchor heuristic will never mark a false deletion and
   show that it cannot catch all deletions. *)

EXTENDS Naturals, Sequences, TLAPS

\* Constants and variables for Board. See Board.tla for an explanation.
CONSTANTS BumpLimit, MaxThreads, Nos
VARIABLES boardDeletedNos, time, unusedNos

\* The set of deleted thread numbers detected by the anchor heuristic
VARIABLE anchorDeletedNos

\* The list of threads from the previous and current polls
VARIABLES prevThreads, currThreads

\* The set of thread numbers we've seen
VARIABLE seenNos

vars == <<anchorDeletedNos, boardDeletedNos, currThreads, prevThreads, seenNos, time, unusedNos>>

Board == INSTANCE Board WITH deletedNos <- boardDeletedNos,
                             threads <- currThreads

Init == /\ Board!Init
        /\ anchorDeletedNos = {}
        /\ prevThreads = <<>>
        /\ seenNos = {}

Last(s) == s[Len(s)]
Range(s) == { s[i] : i \in DOMAIN s }
ThreadNos(s) == { s[i].no : i \in 1..Len(s) }

\* Poll the current threads and use the anchor to find deleted threads
Poll == /\ prevThreads /= currThreads
        /\ (Len(currThreads) > 0 /\ Len(prevThreads) > 0)
        (* If there is at least one unchanged thread, we know the last thread is a valid anchor.
           (See the comments in board_poller.rs for a proof.) This lets us find an anchor even when
           the last thread is saged (i.e. lastModified is updated by Modify). *)
        /\ (Range(currThreads) \intersect Range(prevThreads)) /= {}
        /\ \E anchor \in 1..Len(prevThreads) :
             /\ prevThreads[anchor].no = Last(currThreads).no
             /\ LET removedNos == ThreadNos(prevThreads) \ ThreadNos(currThreads)
                    deletedNos == removedNos \intersect { prevThreads[i].no : i \in 1..anchor - 1 }
                 IN anchorDeletedNos' = anchorDeletedNos \union deletedNos
        /\ prevThreads' = currThreads
        /\ seenNos' = seenNos \union ThreadNos(currThreads)
        /\ UNCHANGED <<boardDeletedNos, currThreads, time, unusedNos>>

\* We couldn't find an anchor, so just copy currThreads to prevThreads
Copy == /\ prevThreads /= currThreads
        /\ prevThreads' = currThreads
        /\ seenNos' = seenNos \union ThreadNos(currThreads)
        /\ UNCHANGED <<anchorDeletedNos, boardDeletedNos, currThreads, time, unusedNos>>

Next == \/ (Poll \/ Copy)
        \/ (Board!Next /\ UNCHANGED <<anchorDeletedNos, prevThreads, seenNos>>)

Spec == Init /\ [][Next]_vars

----

\* We can prove that we will never make a false deletion.
NoFalseDeletions == anchorDeletedNos \subseteq boardDeletedNos

(* It is eventually true that the sets of deleted thread numbers will always be
   equal. That is, we will detect as many deleted threads as possible. We would
   like to be able to prove this, but the anchor heuristic cannot do this. *)
AllDeletions == <>[](anchorDeletedNos = seenNos \intersect boardDeletedNos)

----

Inv == NoFalseDeletions

THEOREM Spec => []NoFalseDeletions
<1>1 Init => Inv
    BY DEF Init, Inv, NoFalseDeletions
<1>2 Inv /\ [Next]_vars => Inv'
    <2> SUFFICES ASSUME Inv, [Next]_vars PROVE Inv'
        OBVIOUS
    <2> USE DEF Inv, NoFalseDeletions
    <2>1 CASE Poll
    (* This is the main crux of the whole proof, but I'm not sure how to do it. Inv needs to be
       expanded, of course, but with what? The current structure might also be awkward for proofs.
       Using a set of threads where position is determined by a bumpTime field and pruned/deleted
       status is determined by another field might be better. *)
    <2>2 CASE Copy
        BY <2>2 DEF Copy
    <2>3 CASE Board!Next /\ UNCHANGED anchorDeletedNos
        <3>1 CASE Board!New \/ Board!Bump \/ Board!Modify
            <4>1 UNCHANGED boardDeletedNos
                BY <3>1 DEF Board!New, Board!Bump, Board!Modify
            <4>2 QED
                BY <2>3, <4>1
        <3>2 CASE Board!Delete
            <4>1 boardDeletedNos \subseteq boardDeletedNos'
                BY <3>2 DEF Board!Delete
            <4>2 QED
                BY <2>3, <4>1
        <3>3 CASE UNCHANGED Board!vars
            BY <2>3, <3>3 DEF Board!vars
        <3>4 QED
            BY <2>3, <3>1, <3>2, <3>3 DEF Board!Next
    <2>4 CASE UNCHANGED vars
        BY <2>4 DEF vars
    <2>5 QED
        BY <2>1, <2>2, <2>3, <2>4 DEF Next
<1>3 Inv => NoFalseDeletions
    BY DEF Inv
<1>4 QED
    BY <1>1, <1>2, <1>3, PTL DEF Spec

====
