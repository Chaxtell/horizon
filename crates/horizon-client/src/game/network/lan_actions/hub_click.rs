//! A hub container click the game owns. `Hub::container_click` used to run inside
//! `self.lan.as_mut()`, so nothing it called could reach the game: a hook fired in the middle
//! of a click had no `&Game` to be given. Here the game lifts the host out of itself for the
//! click, so the click code and the game are separate values and the game is whole while the
//! hub runs.
use super::*;
use crate::game::native_menus::open_menu_of;

impl Game<'_> {
    /// Run `f` with the LAN host taken out of `self.lan`, then put it back. `f` is given the whole
    /// game (`self.lan` is `None` while it runs) and the host. The host is restored on every
    /// path, including a panic in `f`, which is then propagated: no borrow of the host outlives
    /// the call and the game is not left without it. `None` when there is no host to lift,
    /// which includes a call made while the host is already lifted.
    pub(in crate::game) fn with_lan_lifted<R>(
        &mut self,
        f: impl FnOnce(&mut Self, &mut LanHost) -> R,
    ) -> Option<R> {
        let mut host = self.lan.take()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self, &mut host)));
        debug_assert!(self.lan.is_none(), "a lifted LAN host was replaced");
        self.lan = Some(host);
        match result {
            Ok(result) => Some(result),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// `ServerGamePacketListenerImpl.handleContainerClick` for the guest `conn`: the hub's
    /// click, run with the host lifted out of the game. Returns the stacks the click threw out.
    pub(super) fn lan_hub_click(&mut self, conn: u32, p: &play::ContainerClick) -> Vec<ItemStack> {
        self.with_lan_lifted(|game, host| {
            // The game is lent to the click as the mods' `menu_slot_rules` (their read-only `Host`).
            // `self.lan` is `None` meanwhile: what a hook reads of the LAN host answers as if none
            // were open.
            let rules = ItemRules(
                world_items!(game),
                &world_stations!(game).enchantments,
                Some(&*game),
            );
            let Some(mut prepared) = host.hub.prepare_click(conn, p, &game.world) else {
                return (Vec::new(), None);
            };
            let before = prepared.container().map(<[_]>::to_vec);
            // The game is whole here: `game` can be read or written while the hub clicks.
            let outcome = host.hub.run_click(&mut prepared, p, &rules);
            let takes = match (&before, prepared.container()) {
                (Some(before), Some(after)) => slot_takes(before, after),
                _ => Vec::new(),
            };
            let heard = if takes.is_empty() {
                None
            } else {
                let uuid = host.hub.guest(conn).map(|g| g.uuid);
                let menu = open_menu_of(host, conn).map(|(_, menu, pos)| (menu.to_owned(), pos));
                uuid.zip(menu)
            };
            let thrown = host.hub.finish_click(prepared, p, &mut game.world, outcome);
            (thrown, heard.map(|heard| (heard, takes)))
        })
        .map(|(thrown, heard)| {
            if let Some(((player, (menu, pos)), takes)) = heard {
                for (slot, taken, left) in takes {
                    self.native_menu_slot_take(player, &menu, pos, slot, &taken, left.as_ref());
                }
            }
            thrown
        })
        .unwrap_or_default()
    }
}

/// The stacks a click took out of a menu's own slots (`Slot.onTake`): each slot whose stack
/// shrank, emptied or became another item, with what left it and what stayed. Slots that only
/// grew or kept their stack are not takes.
fn slot_takes(
    before: &[Option<ItemStack>],
    after: &[Option<ItemStack>],
) -> Vec<(i32, ItemStack, Option<ItemStack>)> {
    let mut takes = Vec::new();
    for (slot, (was, now)) in before.iter().zip(after).enumerate() {
        let Some(was) = was else { continue };
        let taken = match now {
            Some(now) if now.same_item_same_components(was) => {
                if now.count >= was.count {
                    continue;
                }
                let mut taken = was.clone();
                taken.count = was.count - now.count;
                taken
            }
            _ => was.clone(),
        };
        takes.push((slot as i32, taken, now.clone()));
    }
    takes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_take_is_what_left_a_slot() {
        let stone = |n| Some(ItemStack::new("minecraft:stone", n));
        let dirt = Some(ItemStack::new("minecraft:dirt", 1));
        let before = [stone(10), stone(10), stone(10), None, stone(5)];
        let after = [stone(4), None, dirt.clone(), stone(3), stone(8)];
        let takes = slot_takes(&before, &after);
        let told: Vec<_> = takes
            .iter()
            .map(|(slot, taken, left)| (*slot, taken.count, left.as_ref().map(|l| l.count)))
            .collect();
        assert_eq!(told, [(0, 6, Some(4)), (1, 10, None), (2, 10, Some(1))]);
    }
}
