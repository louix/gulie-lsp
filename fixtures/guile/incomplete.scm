;;; Deliberately unbalanced: the reader must survive this.
(define-module (incomplete)
  #:export (partial))

(define (partial x)
  (let ((y (string-append "unterminated
