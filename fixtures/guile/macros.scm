;;; syntax-rules, syntax-case, and define-syntax-rule definitions.
(define-module (macros)
  #:export (swap! unless* while))

(define-syntax swap!
  (syntax-rules ()
    ((_ a b) (let ((tmp a)) (set! a b) (set! b tmp)))))

(define-syntax-rule (unless* test body ...)
  (if test #f (begin body ...)))

(define-syntax while
  (lambda (stx)
    (syntax-case stx ()
      ((_ test body ...)
       #'(let loop () (when test body ... (loop)))))))
