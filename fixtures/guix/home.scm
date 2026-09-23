;;; Sanitized Guix Home declaration: a shell, dotfiles, and a couple of
;;; services, mirroring the shape of a real one.
(use-modules (gnu home)
             (gnu home services)
             (gnu home services shells)
             (gnu home services ssh)
             (gnu packages)
             (gnu services)
             (guix gexp))

(home-environment
  (packages (specifications->packages (list "git" "ripgrep")))
  (services
   (list (service home-bash-service-type
                  (home-bash-configuration
                   (guix-defaults? #t)
                   (aliases '(("ll" . "ls -l")))
                   (bashrc (list (plain-file "prompt" "PS1='\\w\\$ '")))))
         (service home-openssh-service-type
                  (home-openssh-configuration
                   (hosts (list (openssh-host (name "build")
                                              (host-name "build.example.org")
                                              (user "alice"))))))
         (simple-service 'extra-env
                         home-environment-variables-service-type
                         '(("EDITOR" . "hx"))))))
