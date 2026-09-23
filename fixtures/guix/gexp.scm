;;; Staged code: gexp with ungexp, ungexp-splicing, and a local-file reference.
(use-modules (guix gexp) (gnu packages base))

(define hello-script
  (program-file "hello"
    #~(begin
        (use-modules (ice-9 format))
        (format #t "coreutils at ~a~%" #$(file-append coreutils "/bin"))
        (for-each display '#$@(list "a" "b"))
        (display #$(local-file "./alice.pub")))))
