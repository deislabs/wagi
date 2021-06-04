(module
    ;; This is the example Hello World WAT from the documentation at
    ;; https://github.com/bytecodealliance/wasmtime/blob/main/docs/WASI-tutorial.md
    ;;
    ;; It has been adapted to send CGI headers.
    (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
    (memory 1)
    (export "memory" (memory 0))

    (data (i32.const 8) "content-type: text/html;charset=UTF-8\n\nOh hi world\n")

    (func $main (export "_start")
        (i32.store (i32.const 0) (i32.const 8))
        (i32.store (i32.const 4) (i32.const 51))

        (call $fd_write
            (i32.const 1)
            (i32.const 0)
            (i32.const 1)
            (i32.const 20)
        )
        drop
    )
)