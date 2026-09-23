;;; A module whose indentation has drifted; format/expected.scm is the
;;; formatted result. Comments, strings, and quoted data must survive.
(define-module (format messy)
  #:use-module (ice-9 match)
  #:use-module (srfi srfi-1)
  #:export (shout
            greet))

(define greeting "Hello,
   world")

(define (shout text)
  "Return TEXT in upper case.
Keeps its own indentation."
  ;; a comment before the body
  (string-append (string-upcase text)
                 "!"))

(define* (greet name #:key (punctuation "!"))
  (let loop ((names (if (list? name)
                        name
                        (list name)))
             (count 0))
    (match names
      (() count)
      ((first . rest)
       (format #t "~a ~a~a~%" greeting first punctuation) ; trailing comment
       (loop rest (+ count 1))))))

(define table
  '((a . 1)
    (b . 2)
    (c . "quoted (not code)")))

(define (main args)
  (when (null? args)
    (display "usage: messy NAME...\n")
    (exit 1))
  (for-each (lambda (name)
              (greet name))
            args))
