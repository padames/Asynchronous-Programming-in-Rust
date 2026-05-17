use std::arch::{asm,naked_asm};

const DEFAULT_STACK_SIZE: usize = 1024 * 1024 * 2;
const MAX_THREADS: usize = 4;
// Raw pointer to the active runtime so helper functions can yield/return
// without threading `&mut Runtime` through every boundary.
static mut RUNTIME: usize = 0;

pub struct Runtime {
    threads: Vec<Thread>,
    current: usize,
}

#[derive(PartialEq, Eq, Debug)]
enum State {
    // Slot can be reused by `spawn`.
    Available,
    // Slot currently owns execution.
    Running,
    // Slot is runnable and waiting for scheduler dispatch.
    Ready,
}

struct Thread {
    stack: Vec<u8>,
    ctx: ThreadContext,
    state: State,
}

#[derive(Debug, Default)]
#[repr(C)]
struct ThreadContext {
    // Saved stack pointer plus callee-saved registers for SysV x86_64.
    rsp: u64,
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbx: u64,
    rbp: u64,
}

impl Thread {
    fn new() -> Self {
        Thread {
            stack: vec![0_u8; DEFAULT_STACK_SIZE],
            ctx: ThreadContext::default(),
            state: State::Available,
        }
    }
}

impl Runtime {
    pub fn new() -> Self {
        let base_thread = Thread {
            stack: vec![0_u8; DEFAULT_STACK_SIZE],
            ctx: ThreadContext::default(),
            state: State::Running,
        };

        let mut threads = vec![base_thread];
        let mut available_threads: Vec<Thread> = (1..MAX_THREADS).map(|_| Thread::new()).collect();
        threads.append(&mut available_threads);

        Runtime {
            threads,
            current: 0,
        }
    }

    pub fn init(&self) {
        unsafe {
            let r_ptr: *const Runtime = self;
            RUNTIME = r_ptr as usize;
        }
    }

    pub fn run(&mut self) -> ! {
        // Scheduler "join" loop: keep dispatching until no `Ready` fiber exists.
        while self.t_yield() {}
        std::process::exit(0);
    }

    fn t_return(&mut self) {
        // Called from `guard` when a worker fiber returns.
        // Mark it reusable, then yield to another ready fiber.
        if self.current != 0 {
            self.threads[self.current].state = State::Available;
            self.t_yield();
        }
    }

    #[inline(never)]
    fn t_yield(&mut self) -> bool {
        let mut pos = self.current;
        // Round-robin scan for next runnable (`Ready`) fiber.
        while self.threads[pos].state != State::Ready {
            pos += 1;
            if pos == self.threads.len() {
                pos = 0;
            }
            if pos == self.current {
                // We looped through all slots and found no runnable work.
                return false;
            }
        }
        // Current `Running` fiber goes back to `Ready` unless it already finished.

        if self.threads[self.current].state != State::Available {
            self.threads[self.current].state = State::Ready;
        }
        // Transfer running state to the selected fiber.

        self.threads[pos].state = State::Running;
        let old_pos = self.current;
        self.current = pos;

        unsafe {
            let old: *mut ThreadContext = &mut self.threads[old_pos].ctx;
            let new: *const ThreadContext = &self.threads[pos].ctx;
            // `switch` saves old context, restores new context, then `ret`s
            // using the new stack (fresh entry or resumed continuation).
            asm!("call switch", in("rdi") old, in("rsi") new, clobber_abi("C"));
        }
        self.threads.len() > 0
    }

    pub fn spawn(&mut self, f: fn()) {
        // `spawn` prepares a runnable fiber; it does not execute `f` immediately.
        let available = self
            .threads
            .iter_mut()
            .find(|t| t.state == State::Available)
            .expect("no available thread.");

        let size = available.stack.len();

        unsafe {
            let s_ptr = available.stack.as_mut_ptr().offset(size as isize);
            let s_ptr = (s_ptr as usize & !15) as *mut u8;
            // Bootstrap stack so first switch+`ret` enters:
            //   1) `f` (fiber entry)
            //   2) `skip` (one extra `ret`)
            //   3) `guard` (marks finished and yields)
            std::ptr::write(s_ptr.offset(-16) as *mut u64, guard as u64);
            std::ptr::write(s_ptr.offset(-24) as *mut u64, skip as u64);
            std::ptr::write(s_ptr.offset(-32) as *mut u64, f as u64);
            available.ctx.rsp = s_ptr.offset(-32) as u64;
        }
        available.state = State::Ready;
    }
} // We close the `impl Runtime` block here

fn guard() {
    // Terminal trampoline for returning worker fibers.
    unsafe {
        let rt_ptr = RUNTIME as *mut Runtime;
        (*rt_ptr).t_return();
    };
}

#[unsafe(naked)]
unsafe extern "C" fn skip() {
    // `f` returns here first, then this returns into `guard`.
    //naked_asm!("ret", options(noreturn))
    naked_asm!("ret")
}

pub fn yield_thread() {
    // Cooperative yield point called from fiber code.
    unsafe {
        let rt_ptr = RUNTIME as *mut Runtime;
        (*rt_ptr).t_yield();
    };
}

#[unsafe(naked)]
#[no_mangle]
#[cfg_attr(target_os = "macos", export_name = "\x01switch")] // see: How-to-MacOS-M.md for explanation
unsafe extern "C" fn switch() {
    // rdi = *mut old_ctx, rsi = *const new_ctx
    // Save old callee-saved registers + rsp, restore new ones, and `ret`
    // into the instruction pointer at the top of the new stack.
    naked_asm!(
        "mov [rdi + 0x00], rsp",
        "mov [rdi + 0x08], r15",
        "mov [rdi + 0x10], r14",
        "mov [rdi + 0x18], r13",
        "mov [rdi + 0x20], r12",
        "mov [rdi + 0x28], rbx",
        "mov [rdi + 0x30], rbp",
        "mov rsp, [rsi + 0x00]",
        "mov r15, [rsi + 0x08]",
        "mov r14, [rsi + 0x10]",
        "mov r13, [rsi + 0x18]",
        "mov r12, [rsi + 0x20]",
        "mov rbx, [rsi + 0x28]",
        "mov rbp, [rsi + 0x30]",
        "ret",
//        options(noreturn)
    );
}

fn main() {
    let mut runtime = Runtime::new();
    runtime.init();

    runtime.spawn(|| {
        println!("THREAD 1 STARTING");
        let id = 1;
        for i in 0..10 {
            println!("thread: {} counter: {}", id, i);
            yield_thread();
        }
        println!("THREAD 1 FINISHED");
    });

    runtime.spawn(|| {
        println!("THREAD 2 STARTING");
        let id = 2;
        for i in 0..15 {
            println!("thread: {} counter: {}", id, i);
            yield_thread();
        }
        println!("THREAD 2 FINISHED");
    });
    // Keeps main alive until all runnable fibers complete.
    runtime.run();
}
