use services::crypto::anoncreds::types::{
    Accumulator,
    AccumulatorPublicKey,
    FullProof,
    NonRevocProof,
    NonRevocProofCList,
    NonRevocProofXList,
    NonRevocProofTauList,
    ProofInput,
    PrimaryEqualProof,
    PrimaryPredicateGEProof,
    PrimaryProof,
    PublicKey,
    RevocationPublicKey
};
use services::crypto::anoncreds::constants::{LARGE_E_START, ITERATION, LARGE_NONCE};
use services::crypto::anoncreds::helpers::{AppendByteArray, get_hash_as_int, bignum_to_group_element};
use services::crypto::wrappers::bn::BigNumber;
use std::collections::{HashMap, HashSet};
use errors::crypto::CryptoError;
use services::crypto::wrappers::pair::{Pair, PointG1};
use services::crypto::anoncreds::issuer::Issuer;

pub struct Verifier {}

impl Verifier {
    pub fn new() -> Verifier {
        Verifier {}
    }

    pub fn generate_nonce(&self) -> Result<BigNumber, CryptoError> {
        BigNumber::rand(LARGE_NONCE)
    }

    pub fn verify(&self, pk: PublicKey, pkr: RevocationPublicKey, accum: Accumulator, accum_pk: AccumulatorPublicKey,
                  proof_input: ProofInput, proof: FullProof, all_revealed_attrs: HashMap<String, BigNumber>,
                  nonce: BigNumber, attr_names: HashSet<String>, params: NonRevocProofXList,
                  proof_c: NonRevocProofCList) -> Result<bool, CryptoError> {
        let mut tau_list: Vec<Vec<u8>> = Vec::new();

        for (schema_key, proof_item) in proof.schema_keys.iter().zip(proof.proofs.iter()) {
            if let Some(ref non_revocation_proof) = proof_item.non_revoc_proof {
                tau_list.extend_from_slice(
                    &Verifier::_verify_non_revocation_proof(&pkr, &accum, &accum_pk, &proof.c_hash,
                                                            &non_revocation_proof, &proof_input,
                                                            &params, &proof_c)?.as_slice()?
                );
            };

            tau_list.append_vec(
                &Verifier::_verify_primary_proof(&pk, &proof_input, &proof.c_hash,
                                                 &proof_item.primary_proof, &all_revealed_attrs, &attr_names)?
            )?;
        }

        let mut values: Vec<Vec<u8>> = Vec::new();

        values.push(nonce.to_bytes()?);
        values.extend_from_slice(&tau_list);
        values.extend_from_slice(&proof.c_list);

        let c_hver = get_hash_as_int(&mut values)?;

        Ok(c_hver == proof.c_hash)
    }

    fn _verify_primary_proof(pk: &PublicKey, proof_input: &ProofInput, c_hash: &BigNumber,
                             primary_proof: &PrimaryProof, all_revealed_attrs: &HashMap<String, BigNumber>,
                             attr_names: &HashSet<String>) -> Result<Vec<BigNumber>, CryptoError> {
        let mut t_hat: Vec<BigNumber> = Verifier::_verify_equality(pk, &primary_proof.eq_proof,
                                                                   c_hash, all_revealed_attrs, attr_names)?;

        for ge_proof in primary_proof.ge_proofs.iter() {
            t_hat.append(&mut Verifier::_verify_ge_predicate(pk, ge_proof, c_hash)?)
        }
        Ok(t_hat)
    }

    fn _verify_equality(pk: &PublicKey, proof: &PrimaryEqualProof, c_h: &BigNumber,
                        all_revealed_attrs: &HashMap<String, BigNumber>, attr_names: &HashSet<String>) -> Result<Vec<BigNumber>, CryptoError> {
        let unrevealed_attr_names: HashSet<String> =
            attr_names
                .difference(&proof.revealed_attr_names)
                .map(|attr| attr.to_owned())
                .collect::<HashSet<String>>();

        let t1: BigNumber = Verifier::calc_teq(&pk, &proof.a_prime, &proof.e, &proof.v, &proof.m,
                                               &proof.m1, &proof.m2, &unrevealed_attr_names)?;

        let mut ctx = BigNumber::new_context()?;
        let mut rar = BigNumber::from_dec("1")?;

        for attr_name in proof.revealed_attr_names.iter() {
            let cur_r = pk.r.get(attr_name)
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in pk.r", attr_name)))?;
            let cur_attr = all_revealed_attrs.get(attr_name)
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in all_revealed_attrs", attr_name)))?;

            rar = cur_r
                .mod_exp(&cur_attr, &pk.n, Some(&mut ctx))?
                .mul(&rar, Some(&mut ctx))?;
        }


        let tmp: BigNumber =
            BigNumber::from_dec("2")?
                .exp(
                    &BigNumber::from_dec(&LARGE_E_START.to_string())?,
                    Some(&mut ctx)
                )?;

        rar = proof.a_prime
            .mod_exp(&tmp, &pk.n, Some(&mut ctx))?
            .mul(&rar, Some(&mut ctx))?;

        let t2: BigNumber = pk.z
            .mod_div(&rar, &pk.n)?
            .mod_exp(&c_h, &pk.n, Some(&mut ctx))?
            .inverse(&pk.n, Some(&mut ctx))?;

        let t: BigNumber = t1
            .mul(&t2, Some(&mut ctx))?
            .modulus(&pk.n, Some(&mut ctx))?;

        Ok(vec![t])
    }

    fn _verify_ge_predicate(pk: &PublicKey, proof: &PrimaryPredicateGEProof, c_h: &BigNumber) -> Result<Vec<BigNumber>, CryptoError> {
        let mut ctx = BigNumber::new_context()?;
        let mut tau_list = Verifier::calc_tge(&pk, &proof.u, &proof.r, &proof.mj,
                                              &proof.alpha, &proof.t)?;

        for i in 0..ITERATION {
            let cur_t = proof.t.get(&i.to_string())
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in proof.t", i)))?;

            tau_list[i] = cur_t
                .mod_exp(&c_h, &pk.n, Some(&mut ctx))?
                .inverse(&pk.n, Some(&mut ctx))?
                .mul(&tau_list[i], Some(&mut ctx))?
                .modulus(&pk.n, Some(&mut ctx))?;
        }

        let delta = proof.t.get("DELTA")
            .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in proof.t", "DELTA")))?;

        tau_list[ITERATION] = pk.z
            .mod_exp(
                &BigNumber::from_dec(&proof.predicate.value.to_string())?,
                &pk.n, Some(&mut ctx))?
            .mul(&delta, Some(&mut ctx))?
            .mod_exp(&c_h, &pk.n, Some(&mut ctx))?
            .inverse(&pk.n, Some(&mut ctx))?
            .mul(&tau_list[ITERATION], Some(&mut ctx))?
            .modulus(&pk.n, Some(&mut ctx))?;

        tau_list[ITERATION + 1] = delta
            .mod_exp(&c_h, &pk.n, Some(&mut ctx))?
            .inverse(&pk.n, Some(&mut ctx))?
            .mul(&tau_list[ITERATION + 1], Some(&mut ctx))?
            .modulus(&pk.n, Some(&mut ctx))?;

        Ok(tau_list)
    }

    pub fn calc_tge(pk: &PublicKey, u: &HashMap<String, BigNumber>, r: &HashMap<String, BigNumber>,
                    mj: &BigNumber, alpha: &BigNumber, t: &HashMap<String, BigNumber>)
                    -> Result<Vec<BigNumber>, CryptoError> {
        let mut tau_list: Vec<BigNumber> = Vec::new();
        let mut ctx = BigNumber::new_context()?;

        for i in 0..ITERATION {
            let cur_u = u.get(&i.to_string())
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in u", i)))?;
            let cur_r = r.get(&i.to_string())
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in r", i)))?;

            let t_tau = pk.z
                .mod_exp(&cur_u, &pk.n, Some(&mut ctx))?
                .mul(
                    &pk.s.mod_exp(&cur_r, &pk.n, Some(&mut ctx))?,
                    Some(&mut ctx)
                )?
                .modulus(&pk.n, Some(&mut ctx))?;

            tau_list.push(t_tau);
        }

        let delta = r.get("DELTA")
            .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in r", "DELTA")))?;


        let t_tau = pk.z
            .mod_exp(&mj, &pk.n, Some(&mut ctx))?
            .mul(
                &pk.s.mod_exp(&delta, &pk.n, Some(&mut ctx))?,
                Some(&mut ctx)
            )?
            .modulus(&pk.n, Some(&mut ctx))?;

        tau_list.push(t_tau);

        let mut q: BigNumber = BigNumber::from_dec("1")?;

        for i in 0..ITERATION {
            let cur_t = t.get(&i.to_string())
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in t", i)))?;
            let cur_u = u.get(&i.to_string())
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in u", i)))?;

            q = cur_t
                .mod_exp(&cur_u, &pk.n, Some(&mut ctx))?
                .mul(&q, Some(&mut ctx))?;
        }

        q = pk.s
            .mod_exp(&alpha, &pk.n, Some(&mut ctx))?
            .mul(&q, Some(&mut ctx))?
            .modulus(&pk.n, Some(&mut ctx))?;

        tau_list.push(q);

        Ok(tau_list)
    }

    pub fn calc_teq(pk: &PublicKey, a_prime: &BigNumber, e: &BigNumber, v: &BigNumber,
                    mtilde: &HashMap<String, BigNumber>, m1tilde: &BigNumber, m2tilde: &BigNumber,
                    unrevealed_attr_names: &HashSet<String>) -> Result<BigNumber, CryptoError> {
        let mut ctx = BigNumber::new_context()?;
        let mut result: BigNumber = BigNumber::from_dec("1")?;

        for k in unrevealed_attr_names.iter() {
            let cur_r = pk.r.get(k)
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in pk.r", k)))?;
            let cur_m = mtilde.get(k)
                .ok_or(CryptoError::InvalidStructure(format!("Value by key '{}' not found in mtilde", k)))?;

            result = cur_r
                .mod_exp(&cur_m, &pk.n, Some(&mut ctx))?
                .mul(&result, Some(&mut ctx))?;
        }

        result = pk.rms
            .mod_exp(&m1tilde, &pk.n, Some(&mut ctx))?
            .mul(&result, Some(&mut ctx))?;

        result = pk.rctxt
            .mod_exp(&m2tilde, &pk.n, Some(&mut ctx))?
            .mul(&result, Some(&mut ctx))?;

        result = a_prime
            .mod_exp(&e, &pk.n, Some(&mut ctx))?
            .mul(&result, Some(&mut ctx))?;

        result = pk.s
            .mod_exp(&v, &pk.n, Some(&mut ctx))?
            .mul(&result, Some(&mut ctx))?
            .modulus(&pk.n, Some(&mut ctx))?;

        Ok(result)
    }

    pub fn _verify_non_revocation_proof(pkr: &RevocationPublicKey, accum: &Accumulator, accum_pk: &AccumulatorPublicKey,
                                        c_hash: &BigNumber, proof: &NonRevocProof,
                                        proof_input: &ProofInput, params: &NonRevocProofXList,
                                        proof_c: &NonRevocProofCList)
                                        -> Result<NonRevocProofTauList, CryptoError> {
        let ch_num_z = bignum_to_group_element(&c_hash)?;

        let t_hat_expected_values = Issuer::_create_tau_list_expected_values(pkr, accum, accum_pk, &proof.c_list)?;
        let t_hat_calc_values = Issuer::_create_tau_list_values(&pkr, &accum, &params, &proof_c)?;

        Ok(NonRevocProofTauList {
            t1: t_hat_expected_values.t1.mul(&ch_num_z)?.add(&t_hat_calc_values.t1)?,
            t2: t_hat_expected_values.t2.mul(&ch_num_z)?.add(&t_hat_calc_values.t2)?,
            t3: t_hat_expected_values.t3.pow(&ch_num_z)?.mul(&t_hat_calc_values.t3)?,
            t4: t_hat_expected_values.t4.pow(&ch_num_z)?.mul(&t_hat_calc_values.t4)?,
            t5: t_hat_expected_values.t5.mul(&ch_num_z)?.add(&t_hat_calc_values.t5)?,
            t6: t_hat_expected_values.t6.mul(&ch_num_z)?.add(&t_hat_calc_values.t6)?,
            t7: t_hat_expected_values.t7.pow(&ch_num_z)?.mul(&t_hat_calc_values.t7)?,
            t8: t_hat_expected_values.t8.pow(&ch_num_z)?.mul(&t_hat_calc_values.t8)?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use services::crypto::anoncreds::types::{Proof};
    use services::crypto::anoncreds::prover;

    #[test]
    fn verify_test() {
        let verifier = Verifier::new();

        let mut all_revealed_attrs = HashMap::new();
        all_revealed_attrs.insert("name".to_string(), BigNumber::from_dec("1139481716457488690172217916278103335").unwrap());

        let nonce = BigNumber::from_dec("150136900874297269339868").unwrap();

        let predicate = prover::mocks::get_gvt_predicate();
        let revealed_attrs = prover::mocks::get_revealed_attrs();

        let proof_input = ProofInput {
            revealed_attrs: revealed_attrs,
            predicates: vec![predicate],
            ts: None,
            pubseq_no: None
        };
        let schema_key = prover::mocks::get_gvt_schema_key();

        let eq_proof = mocks::get_eq_proof().unwrap();
        let ge_proof = mocks::get_ge_proof().unwrap();
        let pk = ::services::crypto::anoncreds::issuer::mocks::get_pk().unwrap();

        let primary_proof = PrimaryProof {
            eq_proof: eq_proof,
            ge_proofs: vec![ge_proof]
        };

        let proof = Proof {
            primary_proof: primary_proof,
            non_revoc_proof: None
        };

        let mut c_list: Vec<Vec<u8>> = Vec::new();
        c_list.push(BigNumber::from_dec("40419298688137869960380469261905532334637639358156591584198474730159922131845236332832025717302613443181736582484815352622543977612852994735900017491040605701377167257840237093127235154905233147231624795995550192527737607707481813233736307936765338317096333960487846640715651848248086837945953304627391859983207411514951469156988685936443758957189790705690990639460733132695525553505807698837031674923144499907591301228015553240722485660599743846214527228665753677346129919027033129697444096042970703607475089467398949054480185324997053077334850238886591657619835566943199882335077289734306701560214493298329372650208").unwrap().to_bytes().unwrap());
        c_list.push(BigNumber::from_dec("47324660473671124619766812292419966979218618321195442620378932643647808062884161914306007419982240044457291065692968166148732382413212489017818981907451810722427822947434701298426390923083851509190004176754308805544221591456757905034099563880547910682773230595375415855727922588298088826548392572988130537249508717978384646013947582546019729481146325021203427278860772516903057439582612008766763139310189576482839673644190743850755863703998143105224320265752122772813607076484126428361088197863213824404833756768819688779202461859342789097743829182212846809717194485567647846915198890325457736010590303357798473896700").unwrap().to_bytes().unwrap());
        c_list.push(BigNumber::from_dec("66450517869982062342267997954977032094273479808003128223349391866956221490486227999714708210796649990670474598595144373853545114810461129311488376523373030855652459048816291000188287472254577785187966494209478499264992271438571724964296278469527432908172064052750006541558566871906132838361892473377520708599782848821918665128705358243638618866198451401258608314504494676177177947997456537352832881339718141901132664969277082920274734598386059889447857289735878564021235996969965313779742103257439235693097049742098377325618673992118875810433536654414222034985875962188702260416140781008765351079345681492041353915517").unwrap().to_bytes().unwrap());
        c_list.push(BigNumber::from_dec("78070105827196661040600041337907173457854153272544487321115604386049561730740327194221314976259005306609156189248394958383576900423218823055146785779218825861357426069962919084354758074120740816717011931695486881373830741590805899909505141118332615581712873355033382526097135102214961582694467049685680521168599662570089045106588071095868679795860083477878392645086886419842393734377034091691861772354369870695105905981921915221671803577058964332747681671537519176296905411380141019477128072347200017918410813327520323098847715450370454307294123150568469231654825506721027060142669757561165103933103053528023034511606").unwrap().to_bytes().unwrap());
        c_list.push(BigNumber::from_dec("83200684536414956340494235687534491849084621311799273540992839950256544160417513543839780900524522144337818273323604172338904806642960330906344496013294511314421085013454657603118717753084155308020373268668810396333088299295804908264158817923391623116540755548965302906724851186886232431450985279429884730164260492598022651383336322153593491103199117187195782444754665111992163534318072330538584638714508386890137616826706777205862989966213285981526090164444190640439286605077153051456582398200856066916720632647408699812551248250054268483664698756596786352565981324521663234607300070180614929105425712839420242514321").unwrap().to_bytes().unwrap());

        let proof = FullProof {
            c_hash: BigNumber::from_dec("90321426117300366618517575493200873441415194969656589575988281157859869553034").unwrap(),
            schema_keys: vec![schema_key],
            proofs: vec![proof],
            c_list: c_list
        };
        let attr_names = mocks::get_attr_names();
        let pkr = prover::mocks::get_public_key_revocation().unwrap();
        let accum = prover::mocks::get_accumulator().unwrap();
        let accum_pk = mocks::get_accum_publick_key().unwrap();
        let params = prover::mocks::get_non_revocation_proof_x_list();
        let proof_c = prover::mocks::get_non_revocation_proof_c_list();

        let res = verifier.verify(pk, pkr, accum, accum_pk, proof_input, proof, all_revealed_attrs,
                                  nonce, attr_names, params, proof_c);

        assert!(res.is_ok());
    }

    #[test]
    fn verify_equlity_test() {
        let proof = mocks::get_eq_proof().unwrap();
        let pk = ::services::crypto::anoncreds::issuer::mocks::get_pk().unwrap();
        let c_h = BigNumber::from_dec("90321426117300366618517575493200873441415194969656589575988281157859869553034").unwrap();

        let mut all_revealed_attrs = HashMap::new();
        all_revealed_attrs.insert("name".to_string(), BigNumber::from_dec("1139481716457488690172217916278103335").unwrap());

        let attr_names = mocks::get_attr_names();

        let res: Result<Vec<BigNumber>, CryptoError> = Verifier::_verify_equality(
            &pk,
            &proof,
            &c_h,
            &all_revealed_attrs,
            &attr_names
        );

        assert!(res.is_ok());
        assert_eq!("8587651374942675536728753067347608709923065423222685438966198646355384235605146057750016685007100765028881800702364440231217947350369743\
    7857804979183199263295761778145588965111459517594719543696782791489766042732025814161437109818972963936021789845879318003605961256519820582781422914\
    97483852459936553097915975160943885654662856194246459692268230399812271607008648333989067502873781526028636897730244216695340964909830792881918581540\
    43873141931971315451530757661716555801069654237014399171221318077704626190288641508984014104319842941642570762210967615676477710700081132170451096239\
    93976701236193875603478579771137394", res.unwrap()[0].to_dec().unwrap());
    }

    #[test]
    fn _verify_ge_predicate_works() {
        let proof = mocks::get_ge_proof().unwrap();
        let c_h = BigNumber::from_dec("90321426117300366618517575493200873441415194969656589575988281157859869553034").unwrap();
        let pk = ::services::crypto::anoncreds::issuer::mocks::get_pk().unwrap();

        let res = Verifier::_verify_ge_predicate(&pk, &proof, &c_h);

        assert!(res.is_ok());
        let res_data = res.unwrap();

        assert_eq!("590677196901723818020415922807296116426887937783467552329163347868728175050285426810380554550521915469309366010293784655561646989461816914001376856160959474\
    724414209525842689549578189455824659628722854086979862112126227427503673036934175777141430158851152801070493790103722897828582782870163648640848483116640936376249697914\
    633137312593554018309295958591096901852088786667038390724116720409279123241545342232722741939277853790638731624274772561371001348651265045334956091681420778381377735879\
    68669689592641726487646825879342092157114737380151398135267202044295696236084701682251092338479916535603864922996074284941502", res_data[0].to_dec().unwrap());

        assert_eq!("543920569174455471552712599639581440766547705711484869326147123041712949811245262311199901062814754524825877546701435180039685252325466998614308056075575752\
    3012229141304994213488418248472205210074847942832434112795278331835277383464971076923322954858384250535611705097886772449075174912745310975145629869588136613587711321262\
    7728458751804045531877233822168791389059182616293449039452340074699209366938385424160688825799810090127647002083194688148464107036527938948376814931919821538192884074388\
    857130767228996607411418624748269121453442291957717517888961515288426522014549478484314078535183196345054464060687989571272", res_data[4].to_dec().unwrap());

        assert_eq!("5291248239406641292396471233645296793027806694289670593845325691604331838238498977162512644007769726817609527208308190348307854043130390623053807510337254881\
    53385441651181164838096995680599793153167424540679236858880383788178608357393234960916139159480841866618336282250341768534336113015828670517732010317195575756736857228019\
    99959821781284558791752968988627903716556541708694042188547572928871840445046338355043889462205730182388607688269913628444534146082714639049648123224230863440138867623776\
    549927089094790233964941899325435455174972634582611070515233787127321158133866337540066814079592094148393576048620611972", res_data[5].to_dec().unwrap());
    }

    #[test]
    fn calc_teg_works() {
        let verifier = Verifier::new();
        let proof = mocks::get_ge_proof().unwrap();
        let pk = ::services::crypto::anoncreds::issuer::mocks::get_pk().unwrap();

        let res = Verifier::calc_tge(&pk, &proof.u, &proof.r, &proof.mj,
                                     &proof.alpha, &proof.t);

        assert!(res.is_ok());

        let res_data = res.unwrap();

        assert_eq!("66763809913905005196685504127801735117197865238790458248607529048879049233469065301125917408730585682472169276319924014654607203248656655401523177550968\
    79005126037514992260570317766093693503820466315473651774235097627461187468560528498637265821197064092074734183979312736841571077239362785443096285343022325743749493\
    115671111253247628251990871764988964166665374208195759750683082601207244879323795625125414213912754126587933035233507317880982815199471233315480695428246221116099530\
    2762582265012461801281742135973017791914100890332877707316728113640973774147232476482160263443368393229756851203511677358619849710094360", res_data[1].to_dec().unwrap());

        assert_eq!("1696893728060613826189455641919714506779750280465195946299906248745222420050846334948115499804146149236210969719663609022008928047696210368681129164314195\
    73961162181255619271925974300906611593381407468871521942852472844008029827907111131222578449896833731023679346466149116169563017889291210126870245249099669006944487937\
    701186090023854916946824876428968293209784770081426960793331644949561007921128739917551308870397017309196194046088818137669808278548338892856171583731467477794490146449\
    84371272994658213772000759824325978473230458194532365204418256638583185120380190225687161021928828234401021859449125311307071", res_data[4].to_dec().unwrap());

        assert_eq!("7393309861349259392630193573257336708857960195548821598928169647822585190694497646718777350819780512754931147438702100908573008083971392605400292392558068639\
    6426790932973170010764749286999115602174793097294839591793292822808780386838139840847178284597133066509806751359097256406292722692372335587138313303601933346125677119170\
    3745548456402537166527941377105628418709499120225110517191272248627626095292045349794519230242306378755919873322083068080833514101587864250782718259987761547941791394977\
    87217811540121982252785628801722587508068009691576296044178037535833166612637915579540102026829676380055826672922204922443", res_data[5].to_dec().unwrap());
    }

    #[test]
    fn calc_teq_works() {
        let proof = mocks::get_eq_proof().unwrap();
        let pk = ::services::crypto::anoncreds::issuer::mocks::get_pk().unwrap();
        let unrevealed_attrs = prover::mocks::get_unrevealed_attrs();

        let res = Verifier::calc_teq(&pk, &proof.a_prime, &proof.e, &proof.v,
                                     &proof.m, &proof.m1, &proof.m2, &unrevealed_attrs
        );

        assert!(res.is_ok());
        assert_eq!("44674566012490574873221338726897300898913972309497258940219569980165585727901128041268469063382008728753943624549705899352321456091543114868302412585283526922\
    48482588030725250950307379112600430281021015407801054038315353187338898917957982724509886210242668120433945426431434030155726888483222722925281121829536918755833970204795\
    18277688063064207469055405971871717892031608853055468434231459862469415223592109268515989593021324862858241499053669862628606497232449247691824831224716135821088977103328\
    37686070090582706144278719293684893116662729424191599602937927245245078018737281020133694291784582308345229012480867237", res.unwrap().to_dec().unwrap());
    }
}

pub mod mocks {
    use super::*;
    use ::services::crypto::anoncreds::prover;

    pub fn get_attr_names() -> HashSet<String> {
        let mut attr_names: HashSet<String> = HashSet::new();
        attr_names.insert("name".to_string());
        attr_names.insert("age".to_string());
        attr_names.insert("height".to_string());
        attr_names.insert("sex".to_string());
        attr_names
    }

    pub fn get_ge_proof() -> Result<PrimaryPredicateGEProof, CryptoError> {
        let mut u = HashMap::new();
        u.insert("3".to_string(), BigNumber::from_dec("8991055448884746937183597583722774762484126625050383332471998457846949141029373442125727754282056746716432451682903479769768810979073516373079900011730658561904955804441830070201")?);
        u.insert("0".to_string(), BigNumber::from_dec("3119202262454581234238204378430624579411334710168862570697460713017731159978676020931526979958444245337314728482384630008014840583008894200291024490955989484910144381416270825034")?);
        u.insert("1".to_string(), BigNumber::from_dec("15518000836072591312584487513042312668531396837108384118443738039943502537464561749838550874453205824891384223838670020857450197084265206790593562375607300810229831781795248272746")?);
        u.insert("2".to_string(), BigNumber::from_dec("14825520448375036868008852928056676407055827587737481734442472562914657791730493564843449537953640698472823089255666508559183853195339338542320239187247714921656011972820165680495")?);

        let mut r = HashMap::new();
        r.insert("3".to_string(), BigNumber::from_dec("1167550272049401879986208522893402310804598464734091634200466392129423083223947805081084530528884868358954909996620252475186022489983411045778042594227739715134711989282499524985320110488413880945529981664361709639820806122583682452503036404728763373201248045691893015110010852379757063328461525233426468857514764722036069158904178265410282906843586731152479716245390735227750422991960772359397820443448680191460821952125514509645145886564188922269624264085160475580804514964397619916759653999513671049924196777087113468144988512960417719152393266552894992285322714901696251664710315454136548433461200202231002410586808552657105706728516271798034029334358544147228606049936435037524531381025620665456890088546982587481")?);
        r.insert("0".to_string(), BigNumber::from_dec("2171447327600461898681893459994311075091382696626274737692544709852701253236804421376958076382402020619134253300345593917220742679092835017076022500855973864844382438540332185636399240848767743775256306580762848493986046436797334807658055576925997185840670777012790272251814692816605648587784323426613630301003579746571336649678357714763941128273025862159957664671610945626170382202342056873023285304345808387951726158704872306035900016749011783867480420800998854987117527975876541158475438393405152741773026550341616888761476445877989444379785612563226680131486775899233053750237483379057705217586225573410360257816090005804925119313735493995305192861301036330809025262997449946935113898554709938543261959225374477075")?);
        r.insert("1".to_string(), BigNumber::from_dec("3407533923994509079922445260572851360802767657194628749769491907793892136495870984243826839220225896118619529161581266999433926347085222629115870923342232719053144390143744050810102224808038416215236832553566711013172199073782742820257909889682618205836240882137941793761945944591631439539425000764465713533076522478368670386820666288924406010336355943518262201405259934614234952964126592210374867434305756945477124161456667354597660261751805125868686764527511228958421917556551368867158045859243933424656693853034751832910802366824624573129457523599814696599411287253040266911475142776766859495751666393668865554821250239426074473894708324406330875647014186109228413419914784738994090638263427510209053496949212198772")?);
        r.insert("2".to_string(), BigNumber::from_dec("376615807259433852994889736265571130722120467111857816971887754558663859714462971707188421230515343999387984197735177426886431376277830270779207802969001925574986158648382233404297833366166880771649557924045749558608142093651421705548864007094298410821850827506796116657011958581079961108367131644360333951829519859638856960948927313849945546613528932570789799649277584112030378539271377025534526299113938027086859429617232980159899286261874751664992426761978572712284693482352940080544009977987614687886895144698432208930945866456583811087222056104304977238806342842107136621744373848258397836622192179796587657390442772422614921141854089119770642649923852479045626615424086862226766993260016650650800970901479317353")?);
        r.insert("DELTA".to_string(), BigNumber::from_dec("1204576405206979680375064721017725873269565442920750053860275824473279578144966505696401529388362488618656880602103746663719014543804181028271885056878992356241850630746057861156554344680578591346709669594164380854748723108090171168846365315480163847141547319673663867587891086140001578226570294284600635554860177021112021218221677503541742648400417051405848715777401449235718828129001371122909809318916605795606301174787694751963509104301818268975054567300992103690013595997066100692742805505022623908866248955309724353017333598591476683281090839126513676860390307767387899158218974766900357521082392372102989396002839389060003178573720443299965136555923047732519831454019881161819607825392645740545819410001935871296")?);

        let mut t = HashMap::new();
        t.insert("3".to_string(), BigNumber::from_dec("83832511302317350174644720338005868487742959910398469815023175597193018639890917887543705415062101786582256768017066777905945250455529792569435063542128440269870355757494523489777576305013971151020301795930610571616963448640783534486881066519012584090452409312729129595716959074161404190572673909049999235573789134838668875246480910001667440875590464739356588846924490130540148723881221509872798683154070397912008198847917146244304739030407870533464478489905826281941434008283229667189082264792381734035454956041612257154896426092221951083981809288053249503709950518771668342922637895684467584044654762057518028814700")?);
        t.insert("0".to_string(), BigNumber::from_dec("17363331019061087402844209719893765371888392521507799534029693411314419650156431062459421604096282340039952269582687900721960971874670054761709293109949110830813780630203308029471950250261299362249372820231198558841826592697963838759408960504585788309222390217432925946851327016608993387530098618165007004227557481762160406061606398711655197702267307202795893150693539328844268725519498759780370661097817433632221804533430784357877040495807116168272952720860492630103774088576448694803769740862452948066783609506217979920299119838909533940158375124964345812560749245376080673497973923586841616454700487914362471202008")?);
        t.insert("1".to_string(), BigNumber::from_dec("89455656994262898696010620361749819360237582245028725962970005737051728267174145415488622733389621460891337449519650169354661297765474368093442921019918627430103490796403713184394321040862188347280121162030527387297914106124615295029438860483643206878385030782115461217026682705339179345799048771007488017061121097664849533202200732993683759185652675229998618989002320091590048075901070991065565826421958646807185596723738384036684650647137579559949478266162844209656689415344016818360348356312264086908726131174312873340317036154962789954493075076421104496622960243079994511377273760209424275802376704240224057017113")?);
        t.insert("2".to_string(), BigNumber::from_dec("89410264446544582460783108256046283919076319065430050325756614584399852372030797406836188839188658589044450904082852710142004660134924756488845128162391217899779712577616690285325130344040888345830793786702389605089886670947913310987447937415013394798653152944186602375622211523989869906842514688368412364643177924764258301720702233619449643601070324239497432310281518069485140179427484578654078080286588210649780194784918635633853990818152978680101738950391705291308278990621417475783919318775532419526399483870315453680012214346133208277396870767376190499172447005639213621681954563685885258611100453847030057210573")?);
        t.insert("DELTA".to_string(), BigNumber::from_dec("17531299058220149467416854489421567897910338960471902975273408583568522392255499968302116890306524687486663687730044248160210339238863476091064742601815037120574733471494286906058476822621292173298642666511349405172455078979126802123773531891625097004911163338483230811323704803366602873408421785889893292223666425119841459293545405397943817131052036368166012943639154162916778629230509814424319368937759879498990977728770262630904002681927411874415760739538041907804807946503694675967291621468790462606280423096949972217261933741626487585406950575711867888842552544895574858154723208928052348208022999454364836959913")?);

        let predicate = prover::mocks::get_gvt_predicate();

        let mj = BigNumber::from_dec("1603425011106247404410993992231356816212687443774810147917707956054468639246061842660922922638282972213339086692783888162583747872610530439675358599658842676000681975294259033921")?;
        let alpha = BigNumber::from_dec("10356391427643160498096100322044181597098497015522243313140952718701540840206124784483254227685815326973121415131868716208997744531667356503588945389793642286002145762891552961662804737699174847630739288154243345749050494830443436382280881466833601915627397601315033369264534756381669075511238130934450573103942299767277725603498732898775126784825329479233488928873905649944203334284969529288341712039042121593832892633719941366126598676503928077684908261211960615121039788257179455497199714100480379742080080363623749544442225600170310016965613238530651846654311018291673656192911252359090044631268913200633654215640107245506757349629342277896334140999154991920063754025485899126293818842601918101509689122011832619551509675197082794490012616416413823359927604558553776550532965415598441778103806673039612795460783658848060332784778084904")?;

        Ok(PrimaryPredicateGEProof { u: u, r: r, mj: mj, alpha: alpha, t: t, predicate: predicate })
    }

    pub fn get_eq_proof() -> Result<PrimaryEqualProof, CryptoError> {
        let mtilde = prover::mocks::get_mtilde()?;
        let predicate = prover::mocks::get_gvt_predicate();

        let a_prime = BigNumber::from_dec("78844788312843933904888269033662162831422304046107077675905006898972188325961502973244613809697759885634089891809903260596596204050337720745582204425029325009022804719252242584040122299621227721199828176761231376551096458193462372191787196647068079526052265156928268144134736182005375490381484557881773286686542404542426808122757946974594449826818670853550143124991683881881113838215414675622341721941313438212584005249213398724981821052915678073798488388669906236343688340695052465960401053524210111298793496466799018612997781887930492163394165793209802065308672404407680589643793898593773957386855704715017263075623")?;
        let e = BigNumber::from_dec("157211048330804559357890763556004205033325190265048652432262377822213198765450524518019378474079954420822601420627089523829180910221666161")?;
        let v = BigNumber::from_dec("1284941348270882857396668346831283261477214348763690683497348697824290862398878189368957036860440621466109067749261102013043934190657143812489958705080669016032522931660500036446733706678652522515950127754450934645211652056136276859874236807975473521456606914069014082991239036433172213010731627604460900655694372427254286535318919513622655843830315487127605220061147693872530746405109346050119002875962452785135042012369674224406878631029359470440107271769428236320166308531422754837805075091788368691034173422556029573001095280381990063052098520390497628832466059617626095893334305279839243726801057118958286768204379145955518934076042328930415723280186456582783477760604150368095698975266693968743996433862121883506028239575396951810130540073342769017977933561136433479399747016313456753154246044046173236103107056336293744927119766084120338151498135676089834463415910355744516788140991012773923718618015121004759889110")?;
        let m1 = BigNumber::from_dec("113866224097885880522899498541789692895180427088521824413896638850295809029417413411152277496349590174605786763072969787168775556353363043323193169646869348691540567047982131578875798814721573306665422753535462043941706296398687162611874398835403372887990167434056141368901284989978738291863881602850122461103")?;
        let m2 = BigNumber::from_dec("1323766290428560718316650362032141006992517904653586088737644821361547649912995176966509589375485991923219004461467056332846596210374933277433111217288600965656096366761598274718188430661014172306546555075331860671882382331826185116501265994994392187563331774320231157973439421596164605280733821402123058645")?;


        let mut revealed_attr_names: HashSet<String> = HashSet::new();
        revealed_attr_names.insert("name".to_string());

        Ok(PrimaryEqualProof {
            revealed_attr_names: revealed_attr_names,
            a_prime: a_prime,
            e: e,
            v: v,
            m: mtilde,
            m1: m1,
            m2: m2
        })
    }

    pub fn get_accum_publick_key() -> Result<AccumulatorPublicKey, CryptoError> {
        Ok(AccumulatorPublicKey::new(
            Pair::pair(&PointG1::new().unwrap(), &PointG1::new().unwrap()).unwrap()
        ))
    }
}