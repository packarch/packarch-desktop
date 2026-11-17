// SPDX-License-Identifier: MIT OR Apache-2.0
//! Animations (cahier des charges § 10.5): horloge, courbes, réglages de
//! `rc.xml`. Une animation ne change que l'image: l'état (focus, position
//! finale, entrées) change tout de suite.

use std::time::Duration;

/// Courbe d'une animation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    /// Ressort amorti (masse 1)
    Spring {
        stiffness: f64,
        damping: f64,
    },
}

impl Curve {
    pub fn parse(s: &str) -> Option<Curve> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "linear" => Curve::Linear,
            "ease-out-cubic" | "ease-out" => Curve::EaseOutCubic,
            "ease-in-out-cubic" | "ease-in-out" => Curve::EaseInOutCubic,
            "spring" => Curve::Spring {
                stiffness: 600.0,
                damping: 40.0,
            },
            _ => return None,
        })
    }

    /// Valeur de la courbe pour une progression `t` de 0 à 1 (le ressort
    /// peut dépasser 1); `secs`: durée totale, pour le ressort
    pub fn apply(self, t: f64, secs: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Curve::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Curve::Spring { stiffness, damping } => {
                if t >= 1.0 {
                    return 1.0;
                }
                let x = t * secs;
                let w = stiffness.max(1.0).sqrt();
                let zeta = damping / (2.0 * w);
                if zeta < 1.0 {
                    let wd = w * (1.0 - zeta * zeta).sqrt();
                    1.0 - (-zeta * w * x).exp() * ((wd * x).cos() + zeta * w / wd * (wd * x).sin())
                } else {
                    // Amortissement critique ou plus: sans rebond
                    1.0 - (-w * x).exp() * (1.0 + w * x)
                }
            }
        }
    }
}

/// Les animations du § 10.5
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    WindowOpen,
    WindowClose,
    Minimize,
    Maximize,
    Menu,
    Osd,
    WorkspaceSwitch,
    SnapPreview,
    Shade,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::WindowOpen,
        Kind::WindowClose,
        Kind::Minimize,
        Kind::Maximize,
        Kind::Menu,
        Kind::Osd,
        Kind::WorkspaceSwitch,
        Kind::SnapPreview,
        Kind::Shade,
    ];

    /// Nom de l'élément dans `<animations>`
    pub fn name(self) -> &'static str {
        match self {
            Kind::WindowOpen => "window-open",
            Kind::WindowClose => "window-close",
            Kind::Minimize => "minimize",
            Kind::Maximize => "maximize",
            Kind::Menu => "menu",
            Kind::Osd => "osd",
            Kind::WorkspaceSwitch => "workspace-switch",
            Kind::SnapPreview => "snap-preview",
            Kind::Shade => "shade",
        }
    }

    fn default_ms(self) -> u32 {
        match self {
            Kind::WindowOpen => 160,
            Kind::WindowClose => 140,
            Kind::Minimize => 180,
            Kind::Maximize => 180,
            Kind::Menu => 100,
            Kind::Osd => 100,
            Kind::WorkspaceSwitch => 220,
            Kind::SnapPreview => 120,
            Kind::Shade => 140,
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        let s = s.to_ascii_lowercase();
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }
}

/// Réglage d'une animation
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    pub enabled: bool,
    pub duration_ms: u32,
    pub curve: Curve,
}

/// Section `<animations>` de `rc.xml`
#[derive(Debug, Clone, PartialEq)]
pub struct Animations {
    pub enabled: bool,
    /// Multiplie toutes les durées
    pub speed: f64,
    pub settings: Vec<(Kind, Setting)>,
}

impl Default for Animations {
    fn default() -> Self {
        Animations {
            enabled: true,
            speed: 1.0,
            settings: Kind::ALL
                .into_iter()
                .map(|k| {
                    (
                        k,
                        Setting {
                            enabled: true,
                            duration_ms: k.default_ms(),
                            curve: Curve::EaseOutCubic,
                        },
                    )
                })
                .collect(),
        }
    }
}

impl Animations {
    pub fn setting_mut(&mut self, kind: Kind) -> &mut Setting {
        &mut self.settings.iter_mut().find(|(k, _)| *k == kind).unwrap().1
    }

    /// Nouvelle animation à l'instant `now`, ou `None` si désactivée
    pub fn start(&self, kind: Kind, now: Duration) -> Option<Anim> {
        let s = self.settings.iter().find(|(k, _)| *k == kind)?.1;
        if !self.enabled || !s.enabled || s.duration_ms == 0 || self.speed <= 0.0 {
            return None;
        }
        let ms = (s.duration_ms as f64 * self.speed).round().max(1.0);
        Some(Anim {
            start: now,
            duration: Duration::from_micros((ms * 1000.0) as u64),
            curve: s.curve,
        })
    }
}

/// Animation en cours
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    pub start: Duration,
    pub duration: Duration,
    pub curve: Curve,
}

impl Anim {
    /// Progression brute de 0 à 1
    pub fn progress(&self, now: Duration) -> f64 {
        let d = now.saturating_sub(self.start).as_secs_f64();
        (d / self.duration.as_secs_f64().max(1e-6)).clamp(0.0, 1.0)
    }

    /// Valeur selon la courbe
    pub fn value(&self, now: Duration) -> f64 {
        self.curve.apply(self.progress(now), self.duration.as_secs_f64())
    }

    pub fn done(&self, now: Duration) -> bool {
        self.progress(now) >= 1.0
    }

    /// Animation qui repart d'une valeur affichée `from` (0 à 1) au lieu de
    /// 0: le début est avancé d'autant
    pub fn resumed_from(mut self, from: f64) -> Anim {
        let from = from.clamp(0.0, 1.0);
        let skip = self.duration.mul_f64(from);
        self.start = self.start.saturating_sub(skip);
        self
    }
}

/// Horloge des animations: le temps écoulé depuis le démarrage, ou une
/// horloge simulée pour les captures reproductibles (§ 10.5, tests)
#[derive(Debug, Default)]
pub struct Clock {
    pub frozen: Option<Duration>,
    /// Décalage par rapport au temps réel, après une remise en marche
    ahead: Duration,
    behind: Duration,
}

impl Clock {
    pub fn now(&self, start: std::time::Instant) -> Duration {
        self.frozen
            .unwrap_or_else(|| (start.elapsed() + self.ahead).saturating_sub(self.behind))
    }

    /// Remet l'horloge en marche à partir de l'instant gelé, sans saut
    pub fn run(&mut self, start: std::time::Instant) {
        if let Some(f) = self.frozen.take() {
            let real = self.now(start);
            if f > real {
                self.ahead += f - real;
            } else {
                self.behind += real - f;
            }
        }
    }
}

/// Interpolation linéaire
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves() {
        for c in [
            Curve::Linear,
            Curve::EaseOutCubic,
            Curve::EaseInOutCubic,
            Curve::parse("spring").unwrap(),
        ] {
            assert!(c.apply(0.0, 0.2).abs() < 1e-9, "{c:?}");
            assert!((c.apply(1.0, 0.2) - 1.0).abs() < 1e-9, "{c:?}");
        }
        assert!(Curve::EaseOutCubic.apply(0.5, 0.1) > 0.5);
        assert!((Curve::EaseInOutCubic.apply(0.5, 0.1) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn anims() {
        let mut a = Animations::default();
        let anim = a.start(Kind::Menu, Duration::from_millis(1000)).unwrap();
        assert_eq!(anim.duration, Duration::from_millis(100));
        assert_eq!(anim.progress(Duration::from_millis(1050)), 0.5);
        assert!(anim.done(Duration::from_millis(1100)));
        let r = anim.resumed_from(0.5);
        assert_eq!(r.progress(Duration::from_millis(1000)), 0.5);
        a.speed = 0.5;
        assert_eq!(
            a.start(Kind::Menu, Duration::ZERO).unwrap().duration,
            Duration::from_millis(50)
        );
        a.setting_mut(Kind::Menu).enabled = false;
        assert!(a.start(Kind::Menu, Duration::ZERO).is_none());
        a.enabled = false;
        assert!(a.start(Kind::WindowOpen, Duration::ZERO).is_none());
    }
}
