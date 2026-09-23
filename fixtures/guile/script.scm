#!/usr/bin/env guile
!#
;;; Standalone script: no define-module, top-level definitions and a main entry.
(use-modules (ice-9 format)
             (srfi srfi-1))

(define greeting "Hello")

(define (shout text)
  "Return TEXT in upper case with an exclamation mark."
  (string-append (string-upcase text) "!"))

(define (main args)
  (for-each (lambda (name)
              (format #t "~a, ~a~%" greeting (shout name)))
            (cdr args)))

(main (command-line))
