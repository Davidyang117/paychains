use {
    crate::{broadcast_stage::BroadcastStage, retransmit_stage::RetransmitStage},
    itertools::Itertools,
    lru::LruCache,
    rand::{Rng, SeedableRng},
    rand_chacha::ChaChaRng,
    paychains_gossip::{
        cluster_info::{compute_retransmit_peers, ClusterInfo},
        contact_info::ContactInfo,
        crds_gossip_pull::CRDS_GOSSIP_PULL_CRDS_TIMEOUT_MS,
        weighted_shuffle::{
            weighted_best, weighted_sample_single, weighted_shuffle, WeightedShuffle,
        },
    },
    paychains_ledger::shred::Shred,
    paychains_runtime::bank::Bank,
    paychains_sdk::{
        clock::{Epoch, Slot},
        feature_set,
        pubkey::Pubkey,
        timing::timestamp,
    },
    paychains_streamer::socket::SocketAddrSpace,
    std::{
        any::TypeId,
        cmp::Reverse,
        collections::HashMap,
        marker::PhantomData,
        net::SocketAddr,
        ops::Deref,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    },
};

#[allow(clippy::large_enum_variant)]
enum NodeId {
    // TVU node obtained through gossip (staked or not).
    ContactInfo(ContactInfo),
    // Staked node with no contact-info in gossip table.
    Pubkey(Pubkey),
}

struct Node {
    node: NodeId,
    stake: u64,
}

pub struct ClusterNodes<T> {
    pubkey: Pubkey, // The local node itself.
    // All staked nodes + other known tvu-peers + the node itself;
    // sorted by (stake, pubkey) in descending order.
    nodes: Vec<Node>,
    // Cumulative stakes (excluding the node itself), used for sampling
    // broadcast peers.
    cumulative_weights: Vec<u64>,
    // Weights and indices for sampling peers. weighted_{shuffle,best} expect
    // weights >= 1. For backward compatibility we use max(1, stake) for
    // weights and exclude nodes with no contact-info.
    index: Vec<(/*weight:*/ u64, /*index:*/ usize)>,
    _phantom: PhantomData<T>,
}

type CacheEntry<T> = Option<(/*as of:*/ Instant, Arc<ClusterNodes<T>>)>;

pub struct ClusterNodesCache<T> {
    // Cache entries are wrapped in Arc<Mutex<...>>, so that, when needed, only
    // one thread does the computations to update the entry for the epoch.
    cache: Mutex<LruCache<Epoch, Arc<Mutex<CacheEntry<T>>>>>,
    ttl: Duration, // Time to live.
}

impl Node {
    #[inline]
    fn pubkey(&self) -> Pubkey {
        match &self.node {
            NodeId::Pubkey(pubkey) => *pubkey,
            NodeId::ContactInfo(node) => node.id,
        }
    }

    #[inline]
    fn contact_info(&self) -> Option<&ContactInfo> {
        match &self.node {
            NodeId::Pubkey(_) => None,
            NodeId::ContactInfo(node) => Some(node),
        }
    }
}

impl<T> ClusterNodes<T> {
    pub(crate) fn num_peers(&self) -> usize {
        self.index.len()
    }

    // A peer is considered live if they generated their contact info recently.
    pub(crate) fn num_peers_live(&self, now: u64) -> usize {
        self.index
            .iter()
            .filter_map(|(_, index)| self.nodes[*index].contact_info())
            .filter(|node| {
                let elapsed = if node.wallclock < now {
                    now - node.wallclock
                } else {
                    node.wallclock - now
                };
                elapsed < CRDS_GOSSIP_PULL_CRDS_TIMEOUT_MS
            })
            .count()
    }
}

impl ClusterNodes<BroadcastStage> {
    pub fn new(cluster_info: &ClusterInfo, stakes: &HashMap<Pubkey, u64>) -> Self {
        new_cluster_nodes(cluster_info, stakes)
    }

    pub(crate) fn get_broadcast_addrs(
        &self,
        shred: &Shred,
        root_bank: &Bank,
        fanout: usize,
        socket_addr_space: &SocketAddrSpace,
    ) -> Vec<SocketAddr> {
        const MAX_CONTACT_INFO_AGE: Duration = Duration::from_secs(2 * 60);
        let shred_seed = shred.seed(self.pubkey, root_bank);
        if !enable_turbine_peers_shuffle_patch(shred.slot(), root_bank) {
            if let Some(node) = self.get_broadcast_peer(shred_seed) {
                if socket_addr_space.check(&node.tvu) {
                    return vec![node.tvu];
                }
            }
            return Vec::default();
        }
        let mut rng = ChaChaRng::from_seed(shred_seed);
        let index = match weighted_sample_single(&mut rng, &self.cumulative_weights) {
            None => return Vec::default(),
            Some(index) => index,
        };
        if let Some(node) = self.nodes[index].contact_info() {
            let now = timestamp();
            let age = Duration::from_millis(now.saturating_sub(node.wallclock));
            if age < MAX_CONTACT_INFO_AGE
                && ContactInfo::is_valid_address(&node.tvu, socket_addr_space)
            {
                return vec![node.tvu];
            }
        }
        let nodes: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.pubkey() != self.pubkey)
            .collect();
        if nodes.is_empty() {
            return Vec::default();
        }
        let mut rng = ChaChaRng::from_seed(shred_seed);
        let nodes = shuffle_nodes(&mut rng, &nodes);
        let (neighbors, children) = compute_retransmit_peers(fanout, 0, &nodes);
        neighbors[..1]
            .iter()
            .filter_map(|node| Some(node.contact_info()?.tvu))
            .chain(
                neighbors[1..]
                    .iter()
                    .filter_map(|node| Some(node.contact_info()?.tvu_forwards)),
            )
            .chain(
                children
                    .iter()
                    .filter_map(|node| Some(node.contact_info()?.tvu)),
            )
            .filter(|addr| ContactInfo::is_valid_address(addr, socket_addr_space))
            .collect()
    }

    /// Returns the root of turbine broadcast tree, which the leader sends the
    /// shred to.
    fn get_broadcast_peer(&self, shred_seed: [u8; 32]) -> Option<&ContactInfo> {
        if self.index.is_empty() {
            None
        } else {
            let index = weighted_best(&self.index, shred_seed);
            match &self.nodes[index].node {
                NodeId::ContactInfo(node) => Some(node),
                NodeId::Pubkey(_) => panic!("this should not happen!"),
            }
        }
    }
}

impl ClusterNodes<RetransmitStage> {
    pub(crate) fn get_retransmit_addrs(
        &self,
        slot_leader: Pubkey,
        shred: &Shred,
        root_bank: &Bank,
        fanout: usize,
    ) -> Vec<SocketAddr> {
        let (neighbors, children) =
            self.get_retransmit_peers(slot_leader, shred, root_bank, fanout);
        // If the node is on the critical path (i.e. the first node in each
        // neighborhood), it should send the packet to tvu socket of its
        // children and also tvu_forward socket of its neighbors. Otherwise it
        // should only forward to tvu_forwards socket of its children.
        if neighbors[0].pubkey() != self.pubkey {
            return children
                .iter()
                .filter_map(|node| Some(node.contact_info()?.tvu_forwards))
                .collect();
        }
        // First neighbor is this node itself, so skip it.
        neighbors[1..]
            .iter()
            .filter_map(|node| Some(node.contact_info()?.tvu_forwards))
            .chain(
                children
                    .iter()
                    .filter_map(|node| Some(node.contact_info()?.tvu)),
            )
            .collect()
    }

    fn get_retransmit_peers(
        &self,
        slot_leader: Pubkey,
        shred: &Shred,
        root_bank: &Bank,
        fanout: usize,
    ) -> (
        Vec<&Node>, // neighbors
        Vec<&Node>, // children
    ) {
        let shred_seed = shred.seed(slot_leader, root_bank);
        if !enable_turbine_peers_shuffle_patch(shred.slot(), root_bank) {
            return self.get_retransmit_peers_compat(shred_seed, fanout, slot_leader);
        }
        // Exclude slot leader from list of nodes.
        let nodes: Vec<_> = if slot_leader == self.pubkey {
            error!("retransmit from slot leader: {}", slot_leader);
            self.nodes.iter().collect()
        } else {
            self.nodes
                .iter()
                .filter(|node| node.pubkey() != slot_leader)
                .collect()
        };
        let mut rng = ChaChaRng::from_seed(shred_seed);
        let nodes = shuffle_nodes(&mut rng, &nodes);
        let self_index = nodes
            .iter()
            .position(|node| node.pubkey() == self.pubkey)
            .unwrap();
        let (neighbors, children) = compute_retransmit_peers(fanout, self_index, &nodes);
        // Assert that the node itself is included in the set of neighbors, at
        // the right offset.
        debug_assert_eq!(neighbors[self_index % fanout].pubkey(), self.pubkey);
        (neighbors, children)
    }

    fn get_retransmit_peers_compat(
        &self,
        shred_seed: [u8; 32],
        fanout: usize,
        slot_leader: Pubkey,
    ) -> (
        Vec<&Node>, // neighbors
        Vec<&Node>, // children
    ) {
        // Exclude leader from list of nodes.
        let (weights, index): (Vec<u64>, Vec<usize>) = if slot_leader == self.pubkey {
            error!("retransmit from slot leader: {}", slot_leader);
            self.index.iter().copied().unzip()
        } else {
            self.index
                .iter()
                .filter(|(_, i)| self.nodes[*i].pubkey() != slot_leader)
                .copied()
                .unzip()
        };
        let index: Vec<_> = {
            let shuffle = weighted_shuffle(weights.into_iter(), shred_seed);
            shuffle.into_iter().map(|i| index[i]).collect()
        };
        let self_index = index
            .iter()
            .position(|i| self.nodes[*i].pubkey() == self.pubkey)
            .unwrap();
        let (neighbors, children) = compute_retransmit_peers(fanout, self_index, &index);
        // Assert that the node itself is included in the set of neighbors, at
        // the right offset.
        debug_assert_eq!(
            self.nodes[neighbors[self_index % fanout]].pubkey(),
            self.pubkey
        );
        let neighbors = neighbors.into_iter().map(|i| &self.nodes[i]).collect();
        let children = children.into_iter().map(|i| &self.nodes[i]).collect();
        (neighbors, children)
    }
}

fn build_cumulative_weights(self_pubkey: Pubkey, nodes: &[Node]) -> Vec<u64> {
    let cumulative_stakes: Vec<_> = nodes
        .iter()
        .scan(0, |acc, node| {
            if node.pubkey() != self_pubkey {
                *acc += node.stake;
            }
            Some(*acc)
        })
        .collect();
    if cumulative_stakes.last() != Some(&0) {
        return cumulative_stakes;
    }
    nodes
        .iter()
        .scan(0, |acc, node| {
            if node.pubkey() != self_pubkey {
                *acc += 1;
            }
            Some(*acc)
        })
        .collect()
}

fn new_cluster_nodes<T: 'static>(
    cluster_info: &ClusterInfo,
    stakes: &HashMap<Pubkey, u64>,
) -> ClusterNodes<T> {
    let self_pubkey = cluster_info.id();
    let nodes = get_nodes(cluster_info, stakes);
    let broadcast = TypeId::of::<T>() == TypeId::of::<BroadcastStage>();
    let cumulative_weights = if broadcast {
        build_cumulative_weights(self_pubkey, &nodes)
    } else {
        Vec::default()
    };
    // For backward compatibility:
    //   * nodes which do not have contact-info are excluded.
    //   * stakes are floored at 1.
    // The sorting key here should be equivalent to
    // paychains_gossip::deprecated::sorted_stakes_with_index.
    // Leader itself is excluded when sampling broadcast peers.
    let index = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.contact_info().is_some())
        .filter(|(_, node)| !broadcast || node.pubkey() != self_pubkey)
        .sorted_by_key(|(_, node)| Reverse((node.stake.max(1), node.pubkey())))
        .map(|(index, node)| (node.stake.max(1), index))
        .collect();
    ClusterNodes {
        pubkey: self_pubkey,
        nodes,
        cumulative_weights,
        index,
        _phantom: PhantomData::default(),
    }
}

// All staked nodes + other known tvu-peers + the node itself;
// sorted by (stake, pubkey) in descending order.
fn get_nodes(cluster_info: &ClusterInfo, stakes: &HashMap<Pubkey, u64>) -> Vec<Node> {
    let self_pubkey = cluster_info.id();
    // The local node itself.
    std::iter::once({
        let stake = stakes.get(&self_pubkey).copied().unwrap_or_default();
        let node = NodeId::from(cluster_info.my_contact_info());
        Node { node, stake }
    })
    // All known tvu-peers from gossip.
    .chain(cluster_info.tvu_peers().into_iter().map(|node| {
        let stake = stakes.get(&node.id).copied().unwrap_or_default();
        let node = NodeId::from(node);
        Node { node, stake }
    }))
    // All staked nodes.
    .chain(
        stakes
            .iter()
            .filter(|(_, stake)| **stake > 0)
            .map(|(&pubkey, &stake)| Node {
                node: NodeId::from(pubkey),
                stake,
            }),
    )
    .sorted_by_key(|node| Reverse((node.stake, node.pubkey())))
    // Since sorted_by_key is stable, in case of duplicates, this
    // will keep nodes with contact-info.
    .dedup_by(|a, b| a.pubkey() == b.pubkey())
    .collect()
}

fn enable_turbine_peers_shuffle_patch(shred_slot: Slot, root_bank: &Bank) -> bool {
    let feature_slot = root_bank
        .feature_set
        .activated_slot(&feature_set::turbine_peers_shuffle::id());
    match feature_slot {
        None => false,
        Some(feature_slot) => {
            let epoch_schedule = root_bank.epoch_schedule();
            let feature_epoch = epoch_schedule.get_epoch(feature_slot);
            let shred_epoch = epoch_schedule.get_epoch(shred_slot);
            feature_epoch < shred_epoch
        }
    }
}

// Shuffles nodes w.r.t their stakes.
// Unstaked nodes will always appear at the very end.
fn shuffle_nodes<'a, R: Rng>(rng: &mut R, nodes: &[&'a Node]) -> Vec<&'a Node> {
    // Nodes are sorted by (stake, pubkey) in descending order.
    let stakes: Vec<u64> = nodes
        .iter()
        .map(|node| node.stake)
        .take_while(|stake| *stake > 0)
        .collect();
    let num_staked = stakes.len();
    let mut out: Vec<_> = WeightedShuffle::new(rng, &stakes)
        .unwrap()
        .map(|i| nodes[i])
        .collect();
    let weights = vec![1; nodes.len() - num_staked];
    out.extend(
        WeightedShuffle::new(rng, &weights)
            .unwrap()
            .map(|i| nodes[i + num_staked]),
    );
    out
}

impl<T> ClusterNodesCache<T> {
    pub fn new(
        // Capacity of underlying LRU-cache in terms of number of epochs.
        cap: usize,
        // A time-to-live eviction policy is enforced to refresh entries in
        // case gossip contact-infos are updated.
        ttl: Duration,
    ) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(cap)),
            ttl,
        }
    }
}

impl<T: 'static> ClusterNodesCache<T> {
    fn get_cache_entry(&self, epoch: Epoch) -> Arc<Mutex<CacheEntry<T>>> {
        let mut cache = self.cache.lock().unwrap();
        match cache.get(&epoch) {
            Some(entry) => Arc::clone(entry),
            None => {
                let entry = Arc::default();
                cache.put(epoch, Arc::clone(&entry));
                entry
            }
        }
    }

    pub(crate) fn get(
        &self,
        shred_slot: Slot,
        root_bank: &Bank,
        working_bank: &Bank,
        cluster_info: &ClusterInfo,
    ) -> Arc<ClusterNodes<T>> {
        let epoch = root_bank.get_leader_schedule_epoch(shred_slot);
        let entry = self.get_cache_entry(epoch);
        // Hold the lock on the entry here so that, if needed, only
        // one thread recomputes cluster-nodes for this epoch.
        let mut entry = entry.lock().unwrap();
        if let Some((asof, nodes)) = entry.deref() {
            if asof.elapsed() < self.ttl {
                return Arc::clone(nodes);
            }
        }
        let epoch_staked_nodes = [root_bank, working_bank]
            .iter()
            .find_map(|bank| bank.epoch_staked_nodes(epoch));
        if epoch_staked_nodes.is_none() {
            inc_new_counter_info!("cluster_nodes-unknown_epoch_staked_nodes", 1);
            if epoch != root_bank.get_leader_schedule_epoch(root_bank.slot()) {
                return self.get(root_bank.slot(), root_bank, working_bank, cluster_info);
            }
            inc_new_counter_info!("cluster_nodes-unknown_epoch_staked_nodes_root", 1);
        }
        let nodes = Arc::new(new_cluster_nodes::<T>(
            cluster_info,
            &epoch_staked_nodes.unwrap_or_default(),
        ));
        *entry = Some((Instant::now(), Arc::clone(&nodes)));
        nodes
    }
}

impl From<ContactInfo> for NodeId {
    fn from(node: ContactInfo) -> Self {
        NodeId::ContactInfo(node)
    }
}

impl From<Pubkey> for NodeId {
    fn from(pubkey: Pubkey) -> Self {
        NodeId::Pubkey(pubkey)
    }
}

impl<T> Default for ClusterNodes<T> {
    fn default() -> Self {
        Self {
            pubkey: Pubkey::default(),
            nodes: Vec::default(),
            cumulative_weights: Vec::default(),
            index: Vec::default(),
            _phantom: PhantomData::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        rand::{seq::SliceRandom, Rng},
        paychains_gossip::{
            crds::GossipRoute,
            crds_value::{CrdsData, CrdsValue},
            deprecated::{
                shuffle_peers_and_index, sorted_retransmit_peers_and_stakes,
                sorted_stakes_with_index,
            },
        },
        paychains_sdk::{signature::Keypair, timing::timestamp},
        paychains_streamer::socket::SocketAddrSpace,
        std::{iter::repeat_with, sync::Arc},
    };

    // Legacy methods copied for testing backward compatibility.

    fn get_broadcast_peers(
        cluster_info: &ClusterInfo,
        stakes: Option<&HashMap<Pubkey, u64>>,
    ) -> (Vec<ContactInfo>, Vec<(u64, usize)>) {
        let mut peers = cluster_info.tvu_peers();
        let peers_and_stakes = stake_weight_peers(&mut peers, stakes);
        (peers, peers_and_stakes)
    }

    fn stake_weight_peers(
        peers: &mut Vec<ContactInfo>,
        stakes: Option<&HashMap<Pubkey, u64>>,
    ) -> Vec<(u64, usize)> {
        peers.dedup();
        sorted_stakes_with_index(peers, stakes)
    }

    fn make_cluster<R: Rng>(
        rng: &mut R,
    ) -> (
        Vec<ContactInfo>,
        HashMap<Pubkey, u64>, // stakes
        ClusterInfo,
    ) {
        let mut nodes: Vec<_> = repeat_with(|| ContactInfo::new_rand(rng, None))
            .take(1000)
            .collect();
        nodes.shuffle(rng);
        let this_node = nodes[0].clone();
        let mut stakes: HashMap<Pubkey, u64> = nodes
            .iter()
            .filter_map(|node| {
                if rng.gen_ratio(1, 7) {
                    None // No stake for some of the nodes.
                } else {
                    Some((node.id, rng.gen_range(0, 20)))
                }
            })
            .collect();
        // Add some staked nodes with no contact-info.
        stakes.extend(repeat_with(|| (Pubkey::new_unique(), rng.gen_range(0, 20))).take(100));
        let cluster_info = ClusterInfo::new(
            this_node,
            Arc::new(Keypair::new()),
            SocketAddrSpace::Unspecified,
        );
        {
            let now = timestamp();
            let mut gossip_crds = cluster_info.gossip.crds.write().unwrap();
            // First node is pushed to crds table by ClusterInfo constructor.
            for node in nodes.iter().skip(1) {
                let node = CrdsData::ContactInfo(node.clone());
                let node = CrdsValue::new_unsigned(node);
                assert_eq!(
                    gossip_crds.insert(node, now, GossipRoute::LocalMessage),
                    Ok(())
                );
            }
        }
        (nodes, stakes, cluster_info)
    }

    #[test]
    fn test_cluster_nodes_retransmit() {
        let mut rng = rand::thread_rng();
        let (nodes, stakes, cluster_info) = make_cluster(&mut rng);
        let this_node = cluster_info.my_contact_info();
        // ClusterInfo::tvu_peers excludes the node itself.
        assert_eq!(cluster_info.tvu_peers().len(), nodes.len() - 1);
        let cluster_nodes = new_cluster_nodes::<RetransmitStage>(&cluster_info, &stakes);
        // All nodes with contact-info should be in the index.
        assert_eq!(cluster_nodes.index.len(), nodes.len());
        // Staked nodes with no contact-info should be included.
        assert!(cluster_nodes.nodes.len() > nodes.len());
        // Assert that all nodes keep their contact-info.
        // and, all staked nodes are also included.
        {
            let cluster_nodes: HashMap<_, _> = cluster_nodes
                .nodes
                .iter()
                .map(|node| (node.pubkey(), node))
                .collect();
            for node in &nodes {
                assert_eq!(cluster_nodes[&node.id].contact_info().unwrap().id, node.id);
            }
            for (pubkey, stake) in &stakes {
                if *stake > 0 {
                    assert_eq!(cluster_nodes[pubkey].stake, *stake);
                }
            }
        }
        let (peers, stakes_and_index) =
            sorted_retransmit_peers_and_stakes(&cluster_info, Some(&stakes));
        assert_eq!(stakes_and_index.len(), peers.len());
        assert_eq!(cluster_nodes.index.len(), peers.len());
        for (i, node) in cluster_nodes
            .index
            .iter()
            .map(|(_, i)| &cluster_nodes.nodes[*i])
            .enumerate()
        {
            let (stake, index) = stakes_and_index[i];
            // Wallclock may be update by ClusterInfo::push_self.
            if node.pubkey() == this_node.id {
                assert_eq!(this_node.id, peers[index].id)
            } else {
                assert_eq!(node.contact_info().unwrap(), &peers[index]);
            }
            assert_eq!(node.stake.max(1), stake);
        }
        let slot_leader = nodes[1..].choose(&mut rng).unwrap().id;
        // Remove slot leader from peers indices.
        let stakes_and_index: Vec<_> = stakes_and_index
            .into_iter()
            .filter(|(_stake, index)| peers[*index].id != slot_leader)
            .collect();
        assert_eq!(peers.len(), stakes_and_index.len() + 1);
        let mut shred_seed = [0u8; 32];
        rng.fill(&mut shred_seed[..]);
        let (self_index, shuffled_peers_and_stakes) =
            shuffle_peers_and_index(&this_node.id, &peers, &stakes_and_index, shred_seed);
        let shuffled_index: Vec<_> = shuffled_peers_and_stakes
            .into_iter()
            .map(|(_, index)| index)
            .collect();
        assert_eq!(this_node.id, peers[shuffled_index[self_index]].id);
        for fanout in 1..200 {
            let (neighbors_indices, children_indices) =
                compute_retransmit_peers(fanout, self_index, &shuffled_index);
            let (neighbors, children) =
                cluster_nodes.get_retransmit_peers_compat(shred_seed, fanout, slot_leader);
            assert_eq!(children.len(), children_indices.len());
            for (node, index) in children.into_iter().zip(children_indices) {
                assert_eq!(*node.contact_info().unwrap(), peers[index]);
            }
            assert_eq!(neighbors.len(), neighbors_indices.len());
            assert_eq!(neighbors[0].pubkey(), peers[neighbors_indices[0]].id);
            for (node, index) in neighbors.into_iter().zip(neighbors_indices).skip(1) {
                assert_eq!(*node.contact_info().unwrap(), peers[index]);
            }
        }
    }

    #[test]
    fn test_cluster_nodes_broadcast() {
        let mut rng = rand::thread_rng();
        let (nodes, stakes, cluster_info) = make_cluster(&mut rng);
        // ClusterInfo::tvu_peers excludes the node itself.
        assert_eq!(cluster_info.tvu_peers().len(), nodes.len() - 1);
        let cluster_nodes = ClusterNodes::<BroadcastStage>::new(&cluster_info, &stakes);
        // All nodes with contact-info should be in the index.
        // Excluding this node itself.
        assert_eq!(cluster_nodes.index.len() + 1, nodes.len());
        // Staked nodes with no contact-info should be included.
        assert!(cluster_nodes.nodes.len() > nodes.len());
        // Assert that all nodes keep their contact-info.
        // and, all staked nodes are also included.
        {
            let cluster_nodes: HashMap<_, _> = cluster_nodes
                .nodes
                .iter()
                .map(|node| (node.pubkey(), node))
                .collect();
            for node in &nodes {
                assert_eq!(cluster_nodes[&node.id].contact_info().unwrap().id, node.id);
            }
            for (pubkey, stake) in &stakes {
                if *stake > 0 {
                    assert_eq!(cluster_nodes[pubkey].stake, *stake);
                }
            }
        }
        let (peers, peers_and_stakes) = get_broadcast_peers(&cluster_info, Some(&stakes));
        assert_eq!(peers_and_stakes.len(), peers.len());
        assert_eq!(cluster_nodes.index.len(), peers.len());
        for (i, node) in cluster_nodes
            .index
            .iter()
            .map(|(_, i)| &cluster_nodes.nodes[*i])
            .enumerate()
        {
            let (stake, index) = peers_and_stakes[i];
            assert_eq!(node.contact_info().unwrap(), &peers[index]);
            assert_eq!(node.stake.max(1), stake);
        }
        for _ in 0..100 {
            let mut shred_seed = [0u8; 32];
            rng.fill(&mut shred_seed[..]);
            let index = weighted_best(&peers_and_stakes, shred_seed);
            let peer = cluster_nodes.get_broadcast_peer(shred_seed).unwrap();
            assert_eq!(*peer, peers[index]);
        }
    }
}
