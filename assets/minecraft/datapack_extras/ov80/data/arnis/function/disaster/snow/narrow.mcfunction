execute store result storage arnis:ds lim.value int 1 run scoreboard players get #lim arnis.ds
data modify storage arnis:ds lim.rule set value "commandModificationBlockLimit"
function arnis:disaster/water/rule with storage arnis:ds lim
data modify storage arnis:ds lim.rule set value "max_block_modifications"
function arnis:disaster/water/rule with storage arnis:ds lim
