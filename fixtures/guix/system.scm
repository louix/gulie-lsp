;;; Sanitized operating-system declaration.  Structure mirrors a real laptop
;;; configuration; identifiers, UUIDs, and hardware details are invented.
(use-modules (gnu)
             (gnu system nss)
             (gnu packages linux)
             (gnu packages admin)
             (gnu services ssh)
             (gnu services networking)
             (gnu services sysctl)
             (gnu services pm)
             (gnu services docker)
             (gnu services xorg)
             (my services))
(use-service-modules desktop networking)
(use-package-modules bootloaders window-management xorg package-management text-editors)

(operating-system
  (host-name "fixture")
  (timezone "Europe/London")
  (locale "en_US.utf8")

  (kernel-arguments (append (list "slab_nomerge" "page_alloc.shuffle=1")
                            %default-kernel-arguments))
  (firmware (list))
  (keyboard-layout (keyboard-layout "gb"))

  (bootloader (bootloader-configuration
                (bootloader grub-efi-removable-bootloader)
                (targets '("/boot/efi"))
                (keyboard-layout keyboard-layout)))

  (file-systems
   (append
    (list (file-system
            (mount-point "/")
            (device (uuid "00000000-0000-4000-8000-000000000001"))
            (type "ext4"))
          (file-system
            (mount-point "/boot/efi")
            (device (uuid "0000-0001" 'fat))
            (type "vfat")))
    %base-file-systems))

  (users (cons (user-account
                 (name "alice")
                 (group "users")
                 (supplementary-groups '("wheel" "netdev" "audio" "video")))
               %base-user-accounts))

  (services
   (cons* (service bluetooth-service-type)
          (service thermald-service-type
                   (thermald-configuration (adaptive? #t)))
          (service tlp-service-type
                   (tlp-configuration
                    (cpu-scaling-governor-on-ac (list "performance"))
                    (cpu-scaling-governor-on-bat (list "powersave"))
                    (cpu-boost-on-ac? #t)
                    (cpu-boost-on-bat? #f)))
          (service openssh-service-type
                   (openssh-configuration
                    (x11-forwarding? #t)
                    (authorized-keys `(("alice" ,(local-file "./alice.pub"))))))
          (service docker-service-type)
          (set-xorg-configuration
           (xorg-configuration (keyboard-layout keyboard-layout)))
          (service my-hello-service-type (my-hello-configuration (name "fixture")))
          (service containerd-service-type)
          (modify-services %desktop-services
            (sysctl-service-type config =>
                                 (sysctl-configuration
                                  (inherit config)
                                  (settings (append '(("net.ipv4.conf.all.rp_filter" . "1"))
                                                    %default-sysctl-settings))))
            (guix-service-type config =>
                               (guix-configuration
                                (inherit config)
                                (substitute-urls (append (list "https://substitutes.example.org")
                                                         %default-substitute-urls)))))))

  (packages (append (list kakoune sway smartmontools) %base-packages))

  ;; Allow resolution of '.local' host names with mDNS.
  (name-service-switch %mdns-host-lookup-nss))
