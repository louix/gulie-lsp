;;; Lexical shadowing: the same names bound at different depths.
(define-module (shadowing)
  #:export (outer))

(define x 1)

(define (outer x)
  (let ((y x)
        (x (* x 2)))
    (let* ((x (+ x y))
           (y (lambda (x) (- x 1))))
      (define (inner x) (y x))
      (list x (inner x) (let loop ((x 0)) (if (> x 3) x (loop (+ x 1))))))))
