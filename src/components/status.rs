use crate::asset::SpriteResource;
use crate::components::char::{
    CharAttributeModifier, CharAttributeModifierCollector, CharAttributes, CharOutlook, Percentage,
};
use crate::components::controller::WorldCoords;
use crate::components::{ApplyForceComponent, AttackComponent, AttackType};
use crate::consts::JobId;
use crate::systems::atk_calc::AttackOutcome;
use crate::systems::render::render_command::RenderCommandCollectorComponent;
use crate::systems::render_sys::RenderDesktopClientSystem;
use crate::systems::{Sex, Sprites, SystemVariables};
use crate::ElapsedTime;
use nalgebra::Isometry2;
use specs::{Entity, LazyUpdate};
use std::any::Any;
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use strum_macros::EnumCount;

pub enum StatusStackingResult {
    DontAddTheNewStatus,
    AddTheNewStatus,
}

pub trait Status: Any {
    fn dupl(&self) -> Box<dyn Status>;
    fn can_target_move(&self) -> bool;
    fn typ(&self) -> StatusType;
    fn can_target_cast(&self) -> bool;
    fn get_render_color(&self) -> [f32; 4];
    fn get_render_size(&self) -> f32;
    fn calc_attribs(&self, modifiers: &mut CharAttributeModifierCollector);
    fn calc_render_sprite<'a>(
        &self,
        job_id: JobId,
        head_index: usize,
        sex: Sex,
        sprites: &'a Sprites,
    ) -> Option<&'a SpriteResource>;
    fn update(
        &mut self,
        self_char_id: Entity,
        char_pos: &WorldCoords,
        system_vars: &mut SystemVariables,
        entities: &specs::Entities,
        updater: &mut specs::Write<LazyUpdate>,
    ) -> StatusUpdateResult;

    fn affect_incoming_damage(&mut self, outcome: AttackOutcome) -> AttackOutcome;
    fn allow_push(&mut self, push: &ApplyForceComponent) -> bool;

    fn render(
        &self,
        char_pos: &WorldCoords,
        system_vars: &SystemVariables,
        render_commands: &mut RenderCommandCollectorComponent,
    );
    // (ElapsedTime, f32) == ends_at, percentage
    fn get_status_completion_percent(&self, now: ElapsedTime) -> Option<(ElapsedTime, f32)>;

    fn stack(&mut self, other: Box<dyn Status>) -> StatusStackingResult;
}

// TODO: should 'Dead' be a status?
#[derive(Debug, EnumCount, Clone, Copy)]
pub enum MainStatuses {
    Mounted,
    Stun,
    Poison,
}

#[derive(Clone)]
struct MountedStatus;

const STATUS_ARRAY_SIZE: usize = 32;
pub struct Statuses {
    statuses: [Option<Arc<Mutex<Box<dyn Status>>>>; STATUS_ARRAY_SIZE],
    first_free_index: usize,
    cached_modifier_collector: CharAttributeModifierCollector,
}

unsafe impl Sync for Statuses {}

unsafe impl Send for Statuses {}

impl Statuses {
    pub fn new() -> Statuses {
        Statuses {
            statuses: Default::default(),
            first_free_index: MAINSTATUSES_COUNT,
            cached_modifier_collector: CharAttributeModifierCollector::new(),
        }
    }

    pub fn allow_push(&mut self, push: &ApplyForceComponent) -> bool {
        let mut allow = true;
        for status in self
            .statuses
            .iter_mut()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            allow &= status.as_ref().unwrap().lock().unwrap().allow_push(push);
        }
        return allow;
    }

    pub fn affect_incoming_damage(&mut self, mut outcome: AttackOutcome) -> AttackOutcome {
        for status in self
            .statuses
            .iter_mut()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            outcome = status
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .affect_incoming_damage(outcome);
        }
        return outcome;
    }

    pub fn update(
        &mut self,
        self_char_id: Entity,
        char_pos: &WorldCoords,
        system_vars: &mut SystemVariables,
        entities: &specs::Entities,
        updater: &mut specs::Write<LazyUpdate>,
    ) -> bool {
        let mut changed = false;
        for status in self
            .statuses
            .iter_mut()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            let result = status.as_ref().unwrap().lock().unwrap().update(
                self_char_id,
                char_pos,
                system_vars,
                entities,
                updater,
            );
            match result {
                StatusUpdateResult::RemoveIt => {
                    *status = None;
                    changed = true;
                }
                StatusUpdateResult::KeepIt => {}
            }
        }
        while self.first_free_index > MAINSTATUSES_COUNT
            && self.statuses[self.first_free_index - 1].is_none()
        {
            self.first_free_index -= 1;
        }
        return changed;
    }

    pub fn render(
        &self,
        char_pos: &WorldCoords,
        system_vars: &SystemVariables,
        render_commands: &mut RenderCommandCollectorComponent,
    ) {
        let mut already_rendered = HashSet::with_capacity(self.statuses.len());
        for status in self.statuses.iter().filter(|it| it.is_some()) {
            let guard = status.as_ref().unwrap().lock().unwrap();
            let type_id = guard.deref().as_ref().type_id();
            if !already_rendered.contains(&type_id) {
                guard.render(char_pos, system_vars, render_commands);
                already_rendered.insert(type_id);
            }
        }
    }

    pub fn get_base_attributes(outlook: &CharOutlook) -> CharAttributes {
        return match outlook {
            CharOutlook::Player { job_id, .. } => match job_id {
                _ => CharAttributes {
                    walking_speed: Percentage(100),
                    attack_range: Percentage(100),
                    attack_speed: Percentage(100),
                    attack_damage: 76,
                    armor: Percentage(10),
                    healing: Percentage(100),
                    hp_regen: Percentage(100),
                    max_hp: 2000,
                    mana_regen: Percentage(100),
                },
            },
            CharOutlook::Monster(monster_id) => match monster_id {
                _ => CharAttributes {
                    walking_speed: Percentage(100),
                    attack_range: Percentage(100),
                    attack_speed: Percentage(100),
                    attack_damage: 76,
                    armor: Percentage(0),
                    healing: Percentage(100),
                    hp_regen: Percentage(100),
                    max_hp: 2000,
                    mana_regen: Percentage(100),
                },
            },
        };
    }

    pub fn calc_attributes(&mut self) -> &CharAttributeModifierCollector {
        self.cached_modifier_collector.clear();
        for status in &mut self
            .statuses
            .iter()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            status
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .calc_attribs(&mut self.cached_modifier_collector);
        }
        return &self.cached_modifier_collector;
    }

    pub fn calc_render_sprite<'a>(
        &self,
        job_id: JobId,
        head_index: usize,
        sex: Sex,
        sprites: &'a Sprites,
    ) -> &'a SpriteResource {
        let mut sprite = {
            let sprites = &sprites.character_sprites;
            &sprites[&job_id][sex as usize]
        };
        for status in &mut self
            .statuses
            .iter()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            if let Some(spr) = status
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .calc_render_sprite(job_id, head_index, sex, sprites)
            {
                sprite = spr;
            }
        }
        return sprite;
    }

    pub fn calc_render_color(&self) -> [f32; 4] {
        let mut ret = [1.0, 1.0, 1.0, 1.0];
        for status in &mut self
            .statuses
            .iter()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            let status_color = status.as_ref().unwrap().lock().unwrap().get_render_color();
            for i in 0..4 {
                ret[i] *= status_color[i];
            }
        }
        return ret;
    }

    pub fn calc_largest_remaining_status_time_percent(&self, now: ElapsedTime) -> Option<f32> {
        let mut ret: Option<(ElapsedTime, f32)> = None;
        for status in &mut self
            .statuses
            .iter()
            .take(self.first_free_index)
            .filter(|it| it.is_some())
        {
            let rem: Option<(ElapsedTime, f32)> = status
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .get_status_completion_percent(now);
            ret = if let Some((status_ends_at, _status_remaining_time)) = rem {
                if let Some((current_ends_at, _current_rem_time)) = ret {
                    if current_ends_at.is_later_than(status_ends_at) {
                        rem
                    } else {
                        ret
                    }
                } else {
                    rem
                }
            } else {
                ret
            };
        }
        return ret.map(|it| it.1);
    }

    pub fn is_mounted(&self) -> bool {
        self.statuses[MainStatuses::Mounted as usize].is_some()
    }

    pub fn is_stunned(&self) -> bool {
        self.statuses[MainStatuses::Stun as usize].is_some()
    }

    pub fn switch_mounted(&mut self) {
        let is_mounted = self.statuses[MainStatuses::Mounted as usize].is_some();
        let value: Option<Arc<Mutex<Box<dyn Status>>>> = if !is_mounted {
            Some(Arc::new(Mutex::new(Box::new(MountedStatus {}))))
        } else {
            None
        };
        self.statuses[MainStatuses::Mounted as usize] = value;
    }

    pub fn add(&mut self, status: Arc<Mutex<Box<dyn Status>>>) {
        if self.first_free_index >= STATUS_ARRAY_SIZE {
            log::error!("There is no more space for new Status!");
            return;
        }
        self.statuses[self.first_free_index] = Some(status);
        self.first_free_index += 1;
    }

    pub fn remove_all(&mut self) {
        for status in self.statuses.iter_mut().take(self.first_free_index) {
            *status = None;
        }
        self.first_free_index = MAINSTATUSES_COUNT;
    }

    pub fn remove(&mut self, status_type: StatusType) {
        for arc_status in self.statuses.iter_mut().take(self.first_free_index) {
            let should_remove = arc_status
                .as_ref()
                .map(|it| it.lock().unwrap().typ() == status_type)
                .unwrap_or(false);
            if should_remove {
                *arc_status = None;
            }
        }
    }

    pub fn remove_main_status(&mut self, status: MainStatuses) {
        self.statuses[status as usize] = None;
    }

    pub unsafe fn hack_cast<T>(boxx: &Box<dyn Status>) -> &T {
        // TODO: I could not get back a PosionStatus struct from a Status trait without unsafe, HELP
        // hacky as hell, nothing guarantees that the first pointer in a Trait is the value pointer
        let raw_object: *const T = std::mem::transmute(boxx);
        return &*raw_object;
    }

    pub fn add_poison(
        &mut self,
        poison_caster_entity_id: Entity,
        started: ElapsedTime,
        until: ElapsedTime,
    ) {
        let new_until = {
            let status = &self.statuses[MainStatuses::Poison as usize];
            if let Some(current_poison) = status {
                let boxx: &Box<dyn Status> = &*current_poison.lock().unwrap();
                unsafe { Statuses::hack_cast::<PoisonStatus>(boxx).until.max(until) }
            } else {
                until
            }
        };

        self.statuses[MainStatuses::Poison as usize] =
            Some(Arc::new(Mutex::new(Box::new(PoisonStatus {
                poison_caster_entity_id,
                started,
                until: new_until,
                next_damage_at: started.add_seconds(1.0),
            }))));
    }
}

pub enum StatusUpdateResult {
    RemoveIt,
    KeepIt,
}

impl Status for MountedStatus {
    fn dupl(&self) -> Box<dyn Status> {
        Box::new(MountedStatus)
    }

    fn can_target_move(&self) -> bool {
        true
    }

    fn typ(&self) -> StatusType {
        StatusType::Supportive
    }

    fn can_target_cast(&self) -> bool {
        true
    }

    fn get_render_color(&self) -> [f32; 4] {
        [1.0, 1.0, 1.0, 1.0]
    }

    fn get_render_size(&self) -> f32 {
        1.0
    }

    fn calc_attribs(&self, modifiers: &mut CharAttributeModifierCollector) {
        // it is applied directly on the base moving speed, since it is called first
        modifiers.change_walking_speed(
            CharAttributeModifier::IncreaseByPercentage(Percentage(200)),
            ElapsedTime(0.0),
            ElapsedTime(0.0),
        );
    }

    fn calc_render_sprite<'a>(
        &self,
        job_id: JobId,
        _head_index: usize,
        sex: Sex,
        sprites: &'a Sprites,
    ) -> Option<&'a SpriteResource> {
        Some(&sprites.mounted_character_sprites[&job_id][sex as usize])
    }

    fn update(
        &mut self,
        _self_char_id: Entity,
        _char_pos: &WorldCoords,
        _system_vars: &mut SystemVariables,
        _entities: &specs::Entities,
        _updater: &mut specs::Write<LazyUpdate>,
    ) -> StatusUpdateResult {
        StatusUpdateResult::KeepIt
    }

    fn affect_incoming_damage(&mut self, outcome: AttackOutcome) -> AttackOutcome {
        outcome
    }

    fn allow_push(&mut self, _push: &ApplyForceComponent) -> bool {
        true
    }

    fn render(
        &self,
        _char_pos: &WorldCoords,
        _system_vars: &SystemVariables,
        _render_commands: &mut RenderCommandCollectorComponent,
    ) {
    }

    fn get_status_completion_percent(&self, _now: ElapsedTime) -> Option<(ElapsedTime, f32)> {
        None
    }

    fn stack(&mut self, _other: Box<dyn Status>) -> StatusStackingResult {
        StatusStackingResult::DontAddTheNewStatus
    }
}

#[derive(Clone)]
pub struct PoisonStatus {
    pub poison_caster_entity_id: Entity,
    pub started: ElapsedTime,
    pub until: ElapsedTime,
    pub next_damage_at: ElapsedTime,
}

impl Status for PoisonStatus {
    fn dupl(&self) -> Box<dyn Status> {
        Box::new(self.clone())
    }

    fn can_target_move(&self) -> bool {
        true
    }

    fn typ(&self) -> StatusType {
        StatusType::Harmful
    }

    fn can_target_cast(&self) -> bool {
        true
    }

    fn get_render_color(&self) -> [f32; 4] {
        [0.5, 1.0, 0.5, 1.0]
    }

    fn get_render_size(&self) -> f32 {
        1.0
    }

    fn calc_attribs(&self, _modifiers: &mut CharAttributeModifierCollector) {}

    fn calc_render_sprite<'a>(
        &self,
        _job_id: JobId,
        _head_index: usize,
        _sex: Sex,
        _sprites: &'a Sprites,
    ) -> Option<&'a SpriteResource> {
        None
    }

    fn update(
        &mut self,
        self_char_id: Entity,
        _char_pos: &WorldCoords,
        system_vars: &mut SystemVariables,
        _entities: &specs::Entities,
        _updater: &mut specs::Write<LazyUpdate>,
    ) -> StatusUpdateResult {
        if self.until.is_earlier_than(system_vars.time) {
            StatusUpdateResult::RemoveIt
        } else {
            if self.next_damage_at.is_earlier_than(system_vars.time) {
                system_vars.attacks.push(AttackComponent {
                    src_entity: self.poison_caster_entity_id,
                    dst_entity: self_char_id,
                    typ: AttackType::Poison(30),
                });
                self.next_damage_at = system_vars.time.add_seconds(1.0);
            }
            StatusUpdateResult::KeepIt
        }
    }

    fn affect_incoming_damage(&mut self, outcome: AttackOutcome) -> AttackOutcome {
        outcome
    }

    fn allow_push(&mut self, _push: &ApplyForceComponent) -> bool {
        true
    }

    fn render(
        &self,
        char_pos: &WorldCoords,
        system_vars: &SystemVariables,
        render_commands: &mut RenderCommandCollectorComponent,
    ) {
        RenderDesktopClientSystem::render_str(
            "quagmire",
            self.started,
            char_pos,
            system_vars,
            render_commands,
        );
    }

    fn get_status_completion_percent(&self, now: ElapsedTime) -> Option<(ElapsedTime, f32)> {
        Some((self.until, now.percentage_between(self.started, self.until)))
    }

    fn stack(&mut self, _other: Box<dyn Status>) -> StatusStackingResult {
        StatusStackingResult::AddTheNewStatus
    }
}

pub enum ApplyStatusComponentPayload {
    MainStatus(MainStatuses),
    SecondaryStatus(Arc<Mutex<Box<dyn Status>>>),
}

impl ApplyStatusComponentPayload {
    pub fn from_main_status(m: MainStatuses) -> ApplyStatusComponentPayload {
        ApplyStatusComponentPayload::MainStatus(m)
    }

    pub fn from_secondary(status: Box<dyn Status>) -> ApplyStatusComponentPayload {
        ApplyStatusComponentPayload::SecondaryStatus(Arc::new(Mutex::new(status)))
    }
}

impl Clone for ApplyStatusComponentPayload {
    fn clone(&self) -> Self {
        match self {
            ApplyStatusComponentPayload::MainStatus(m) => {
                ApplyStatusComponentPayload::MainStatus(*m)
            }
            ApplyStatusComponentPayload::SecondaryStatus(arc) => {
                let boxed_status_clone = arc.lock().unwrap().dupl();
                ApplyStatusComponentPayload::SecondaryStatus(Arc::new(Mutex::new(
                    boxed_status_clone,
                )))
            }
        }
    }
}

pub struct ApplyStatusComponent {
    pub source_entity_id: Entity,
    pub target_entity_id: Entity,
    pub status: ApplyStatusComponentPayload,
}

pub struct ApplyStatusInAreaComponent {
    pub source_entity_id: Entity,
    pub status: ApplyStatusComponentPayload,
    pub area_shape: Box<dyn ncollide2d::shape::Shape<f32>>,
    pub area_isom: Isometry2<f32>,
    pub except: Option<Entity>,
}

#[derive(Eq, PartialEq, Clone, Copy)]
pub enum StatusType {
    Supportive,
    Harmful,
}

pub enum RemoveStatusComponentPayload {
    MainStatus(MainStatuses),
    SecondaryStatus(StatusType),
}

pub struct RemoveStatusComponent {
    pub source_entity_id: Entity,
    pub target_entity_id: Entity,
    pub status: RemoveStatusComponentPayload,
}

unsafe impl Sync for ApplyStatusComponent {}

unsafe impl Send for ApplyStatusComponent {}

unsafe impl Sync for ApplyStatusInAreaComponent {}

unsafe impl Send for ApplyStatusInAreaComponent {}

impl ApplyStatusComponent {
    pub fn from_main_status(
        source_entity_id: Entity,
        target_entity_id: Entity,
        m: MainStatuses,
    ) -> ApplyStatusComponent {
        ApplyStatusComponent {
            source_entity_id,
            target_entity_id,
            status: ApplyStatusComponentPayload::MainStatus(m),
        }
    }

    pub fn from_secondary_status(
        source_entity_id: Entity,
        target_entity_id: Entity,
        status: Box<dyn Status>,
    ) -> ApplyStatusComponent {
        ApplyStatusComponent {
            source_entity_id,
            target_entity_id,
            status: ApplyStatusComponentPayload::from_secondary(status),
        }
    }
}

impl RemoveStatusComponent {
    pub fn from_main_status(
        source_entity_id: Entity,
        target_entity_id: Entity,
        m: MainStatuses,
    ) -> RemoveStatusComponent {
        RemoveStatusComponent {
            source_entity_id,
            target_entity_id,
            status: RemoveStatusComponentPayload::MainStatus(m),
        }
    }

    pub fn from_secondary_status(
        source_entity_id: Entity,
        target_entity_id: Entity,
        status_type: StatusType,
    ) -> RemoveStatusComponent {
        RemoveStatusComponent {
            source_entity_id,
            target_entity_id,
            status: RemoveStatusComponentPayload::SecondaryStatus(status_type),
        }
    }
}
