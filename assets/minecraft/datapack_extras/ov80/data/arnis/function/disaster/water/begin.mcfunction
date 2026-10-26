scoreboard players set #len arnis.ds 0
scoreboard players set #wv_on arnis.ds 1
function arnis:disaster/water/hold
scoreboard players set #wv_lv arnis.ds 1
scoreboard players set #wv_t arnis.ds 0
scoreboard players set #wv_dir arnis.ds 1
execute if score #wv_max arnis.ds matches ..1 run scoreboard players set #wv_dir arnis.ds 0
data modify storage arnis:ds status set value "rising"
execute if score #wv_dir arnis.ds matches 0 run data modify storage arnis:ds status set value "at its peak"
