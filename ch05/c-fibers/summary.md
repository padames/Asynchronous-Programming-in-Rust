Stackful Fibers Runtime Analysis (ch05/c-fibers/src/main.rs)

High-level model
This runtime is a single OS thread with multiple user-space fibers.  
Scheduling is cooperative: a fiber runs until it explicitly yields (or returns).


Why both spawn closures run
runtime.spawn(...) does not execute the closure immediately.  
It only:

1. picks an Available fiber slot,
2. builds a synthetic stack/context for that function,
3. marks the slot Ready.

So after two spawn calls, both fibers are queued as runnable.  
Execution of those closures starts only when runtime.run() begins scheduling.

Scheduler loop (run)
run() is the driver/join loop:
text
It repeatedly asks scheduler logic (t_yield) to switch to another runnable fiber.  
When no Ready fiber exists, t_yield() returns false, loop ends, and program exits.

So yes: control reaches run() before spawned closures finish (before they even start).

The three t_yield call sites
There are three paths into the same scheduler primitive:

1. `run()` loop  
   Keeps runtime alive and dispatches runnable fibers.
2. `yield_thread()` inside closure bodies  
   Voluntary handoff to another ready fiber.
3. `t_return()` (called from `guard`)  
   Used when a fiber function returns; marks fiber done (Available) and yields away.

These cooperate to interleave execution and eventually drain all work.

Assembly context switch (switch) mechanics
`t_yield()` picks old and new, then calls:

 *  switch(old_ctx, new_ctx)

Inside switch:

1. Save old context: rsp, r15, r14, r13, r12, rbx, rbp.
2. Restore new context: same registers from new_ctx.
3. Execute ret.

The critical trick: after loading new rsp, ret pops the instruction pointer from the new fiber’s stack:

 *  first run of a spawned fiber: lands at closure entry f,
 *  later resumes: lands at continuation point after a prior yield.

Bootstrap return chain for a fresh fiber
spawn seeds stack so first execution and termination flow are:

1. ret -> f (closure body starts)
2. closure returns: ret -> skip
3. skip immediately ret -> guard
4. guard -> t_return()
5. t_return() marks current fiber Available and yields to another ready fiber

So completion is also integrated into scheduler control flow.

Concrete cycle example
Initial state after spawning:

 *  T0 = Running (runtime/main fiber)
 *  T1 = Ready
 *  T2 = Ready

Cycle:

1. T0 (run) calls t_yield -> switches to T1.
2. T1 runs, calls yield_thread -> switches to T2.
3. T2 runs, calls yield_thread -> switches back to T0.
4. T0 resumes inside run loop, calls t_yield again, repeats.

Final termination sequence
Near end:

 * workers eventually return, enter guard -> t_return,
 * each finished fiber becomes Available,
 * a final switch returns execution to T0 (run),
 * next t_yield scan finds no Ready fibers -> returns false,
 * run loop ends and process exits.

Fiber state lifecycle
For worker fibers:

Available -> Ready -> Running -> Ready -> ... -> Running -> Available

 * spawn: Available -> Ready
 * scheduler dispatch: Ready -> Running
 * voluntary yield: Running -> Ready
 * function return (t_return): Running -> Available
